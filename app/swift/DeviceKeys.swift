// The person's device keys in this Mac's Secure Enclave, for the Jaynes app (src/device_key.rs), and nothing else: no
// network, no keychain item. A key's sealed block goes back to the app, which keeps it in a file; the block is of use
// only on this Mac, and signing with it takes the person's gesture. Built into the app by build.rs (a static library),
// so the code that signs is the app's own, under its signature.
import CryptoKit
import Foundation
import LocalAuthentication
import Security

private func put(_ data: Data, _ out: UnsafeMutablePointer<UInt8>, _ cap: Int, _ len: UnsafeMutablePointer<Int>) -> Bool {
    guard data.count <= cap else { return false }
    data.copyBytes(to: out, count: data.count)
    len.pointee = data.count
    return true
}

private func say(_ s: String, _ out: UnsafeMutablePointer<UInt8>, _ cap: Int) {
    let bytes = Array(s.utf8.prefix(max(cap - 1, 0)))
    for (i, b) in bytes.enumerated() { out[i] = b }
    if cap > 0 { out[bytes.count] = 0 }
}

/// 0: no Secure Enclave; 1: one, and no finger enrolled for Touch ID; 2: one, and Touch ID.
@_cdecl("jaynes_se_available")
public func jaynesSeAvailable() -> Int32 {
    guard SecureEnclave.isAvailable else { return 0 }
    var err: NSError?
    return LAContext().canEvaluatePolicy(.deviceOwnerAuthenticationWithBiometrics, error: &err) ? 2 : 1
}

/// A new key, only while the Mac is unlocked: with `biometry` it signs only with a finger enrolled now, else with the
/// person's finger or their password. Its sealed block and its public key (X9.63). 0, or 1 and why.
@_cdecl("jaynes_se_create")
public func jaynesSeCreate(
    _ biometry: Int32,
    _ block: UnsafeMutablePointer<UInt8>, _ blockCap: Int, _ blockLen: UnsafeMutablePointer<Int>,
    _ pub: UnsafeMutablePointer<UInt8>, _ pubCap: Int, _ pubLen: UnsafeMutablePointer<Int>,
    _ why: UnsafeMutablePointer<UInt8>, _ whyCap: Int
) -> Int32 {
    let flags: SecAccessControlCreateFlags = biometry != 0 ? [.privateKeyUsage, .biometryCurrentSet] : [.privateKeyUsage, .userPresence]
    var cfErr: Unmanaged<CFError>?
    guard let access = SecAccessControlCreateWithFlags(nil, kSecAttrAccessibleWhenUnlockedThisDeviceOnly, flags, &cfErr) else {
        say("the access of the key: \(cfErr.map { "\($0.takeRetainedValue())" } ?? "")", why, whyCap)
        return 1
    }
    do {
        let key = try SecureEnclave.P256.Signing.PrivateKey(accessControl: access)
        guard put(key.dataRepresentation, block, blockCap, blockLen), put(key.publicKey.x963Representation, pub, pubCap, pubLen) else {
            say("the key does not fit what the app gave", why, whyCap)
            return 1
        }
        return 0
    } catch {
        say("\(error)", why, whyCap)
        return 1
    }
}

/// The public key (X9.63) of a sealed block, with no prompt. 0, or 1 and why.
@_cdecl("jaynes_se_public")
public func jaynesSePublic(
    _ block: UnsafePointer<UInt8>, _ blockLen: Int,
    _ pub: UnsafeMutablePointer<UInt8>, _ pubCap: Int, _ pubLen: UnsafeMutablePointer<Int>,
    _ why: UnsafeMutablePointer<UInt8>, _ whyCap: Int
) -> Int32 {
    do {
        let key = try SecureEnclave.P256.Signing.PrivateKey(dataRepresentation: Data(bytes: block, count: blockLen))
        guard put(key.publicKey.x963Representation, pub, pubCap, pubLen) else { say("the key does not fit", why, whyCap); return 1 }
        return 0
    } catch {
        say("\(error)", why, whyCap)
        return 1
    }
}

/// The person's gesture once (Touch ID, or their password if they choose it), then `message` signed (DER): by the
/// device key always, and by the fingerprint key, when there is one, only if the gesture was a finger. That one is
/// asked with no interaction, so a password never opens a second dialog (the tech-lead's decision, 18/11). 0: signed
/// (`bioSigLen` 0 when the fingerprint key did not sign); 2: the person cancelled; 1: anything else, and why.
@_cdecl("jaynes_se_sign")
public func jaynesSeSign(
    _ presence: UnsafePointer<UInt8>, _ presenceLen: Int,
    _ bio: UnsafePointer<UInt8>?, _ bioLen: Int,
    _ message: UnsafePointer<UInt8>, _ messageLen: Int,
    _ reason: UnsafePointer<CChar>,
    _ presenceSig: UnsafeMutablePointer<UInt8>, _ presenceCap: Int, _ presenceSigLen: UnsafeMutablePointer<Int>,
    _ bioSig: UnsafeMutablePointer<UInt8>, _ bioCap: Int, _ bioSigLen: UnsafeMutablePointer<Int>,
    _ why: UnsafeMutablePointer<UInt8>, _ whyCap: Int
) -> Int32 {
    let text = String(cString: reason)
    let ctx = LAContext()
    ctx.localizedReason = text
    let done = DispatchSemaphore(value: 0)
    var passed = false
    var failure: Error?
    ctx.evaluatePolicy(.deviceOwnerAuthentication, localizedReason: text) { ok, err in
        passed = ok
        failure = err
        done.signal()
    }
    done.wait()
    bioSigLen.pointee = 0
    guard passed else {
        let code = (failure as? LAError)?.code
        if code == .userCancel || code == .appCancel || code == .systemCancel { return 2 }
        say("no gesture: \(failure.map { "\($0)" } ?? "")", why, whyCap)
        return 1
    }
    let data = Data(bytes: message, count: messageLen)
    do {
        let key = try SecureEnclave.P256.Signing.PrivateKey(dataRepresentation: Data(bytes: presence, count: presenceLen), authenticationContext: ctx)
        guard put(try key.signature(for: data).derRepresentation, presenceSig, presenceCap, presenceSigLen) else {
            say("the signature does not fit", why, whyCap)
            return 1
        }
    } catch {
        say("the device key did not sign: \(error)", why, whyCap)
        return 1
    }
    if let bio = bio, bioLen > 0 {
        ctx.interactionNotAllowed = true
        if let key = try? SecureEnclave.P256.Signing.PrivateKey(dataRepresentation: Data(bytes: bio, count: bioLen), authenticationContext: ctx),
           let s = try? key.signature(for: data) {
            _ = put(s.derRepresentation, bioSig, bioCap, bioSigLen)
        }
    }
    return 0
}
