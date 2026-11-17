// Jaynes Input: real mouse and keyboard events for a test of the Jaynes app. Keys go to the app's own process; the
// mouse goes through the system, as a mouse does (a mouse event posted to a process reaches no window), so the cursor
// moves for the test and goes back where it was at the end of the script. macOS lets a process post events only with the Accessibility permission;
// this small app holds it alone, so no agent's terminal needs it.
//
// open -W -n "Jaynes Input.app" --args <script> <out>
// The script, a command per line:
//   trusted                       whether the permission is given (writes true or false)
//   ask                           asks macOS for the permission (System Settings opens)
//   window <pid>                  the pid's main window on screen: x y width height, in points
//   key <pid> <keys>              a key with its modifiers, as cmd-w, cmd-shift-], cmd-alt-down, escape
//   click <pid> <x> <y>           a left click at that point of the screen
//   drag <pid> <x1> <y1> <x2> <y2>  press at one point, move to the other, release
//   front <pid>                   the pid's app in front; writes the app that was in front before, by bundle id
//   activate <bundle-id>          that app in front again
//   sleep <ms>
import AppKit
import ApplicationServices

let args = CommandLine.arguments
guard args.count >= 3, let script = try? String(contentsOfFile: args[1], encoding: .utf8) else { exit(2) }
var out = ""
func say(_ s: String) { out += s + "\n" }

let codes: [String: CGKeyCode] = [
    "a": 0, "s": 1, "d": 2, "f": 3, "h": 4, "g": 5, "z": 6, "x": 7, "c": 8, "v": 9, "b": 11, "q": 12, "w": 13, "e": 14,
    "r": 15, "y": 16, "t": 17, "1": 18, "2": 19, "3": 20, "4": 21, "6": 22, "5": 23, "=": 24, "9": 25, "7": 26, "-": 27,
    "8": 28, "0": 29, "]": 30, "o": 31, "u": 32, "[": 33, "i": 34, "p": 35, "enter": 36, "l": 37, "j": 38, "'": 39,
    "k": 40, ";": 41, "\\": 42, ",": 43, "/": 44, "n": 45, "m": 46, ".": 47, "tab": 48, "space": 49, "escape": 53,
    "left": 123, "right": 124, "down": 125, "up": 126,
]

func key(_ pid: pid_t, _ combo: String) {
    var parts = combo.split(separator: "-", omittingEmptySubsequences: false).map(String.init)
    // "cmd--" is cmd and the minus key
    if combo.hasSuffix("--") { parts = Array(parts.dropLast(2)) + ["-"] }
    guard let name = parts.last, let code = codes[name] else { say("no key \(combo)"); return }
    var flags: CGEventFlags = []
    for m in parts.dropLast() {
        switch m {
        case "cmd": flags.insert(.maskCommand)
        case "shift": flags.insert(.maskShift)
        case "alt": flags.insert(.maskAlternate)
        case "ctrl": flags.insert(.maskControl)
        default: say("no modifier \(m)"); return
        }
    }
    let src = CGEventSource(stateID: .hidSystemState)
    for down in [true, false] {
        let e = CGEvent(keyboardEventSource: src, virtualKey: code, keyDown: down)!
        e.flags = flags
        e.postToPid(pid)
        usleep(20_000)
    }
    say("key \(combo)")
}

/// The pid's main window: its number and bounds, the largest of its windows at the normal level.
func mainWindow(_ pid: pid_t) -> (Int, [String: Double])? {
    let list = CGWindowListCopyWindowInfo([.optionAll], kCGNullWindowID) as? [[String: Any]] ?? []
    return list
        .filter { ($0[kCGWindowOwnerPID as String] as? pid_t) == pid && ($0[kCGWindowLayer as String] as? Int) == 0 }
        .compactMap { w -> (Int, [String: Double])? in
            guard let n = w[kCGWindowNumber as String] as? Int, let b = w[kCGWindowBounds as String] as? [String: Double] else { return nil }
            return (n, b)
        }
        .max { ($0.1["Width"]! * $0.1["Height"]!) < ($1.1["Width"]! * $1.1["Height"]!) }
}

/// Where the cursor was when the script began, to put it back at the end.
let cursorWas = CGEvent(source: nil)?.location

/// A mouse event through the system, as a mouse makes it: it reaches whatever window is at that point.
func mouse(_ pid: pid_t, _ type: CGEventType, _ x: Double, _ y: Double) {
    let e = CGEvent(mouseEventSource: CGEventSource(stateID: .hidSystemState), mouseType: type, mouseCursorPosition: CGPoint(x: x, y: y), mouseButton: .left)!
    if type == .leftMouseDown || type == .leftMouseUp { e.setIntegerValueField(.mouseEventClickState, value: 1) }
    e.post(tap: .cghidEventTap)
}

func window(_ pid: pid_t) {
    guard let (_, b) = mainWindow(pid) else { say("no window"); return }
    say("window \(Int(b["X"]!)) \(Int(b["Y"]!)) \(Int(b["Width"]!)) \(Int(b["Height"]!))")
}

for line in script.split(separator: "\n") {
    let w = line.split(separator: " ").map(String.init)
    guard let verb = w.first else { continue }
    let pid = w.count > 1 ? pid_t(w[1]) ?? 0 : 0
    let n = w.dropFirst(2).compactMap(Double.init)
    switch verb {
    case "trusted": say(AXIsProcessTrusted() ? "true" : "false")
    case "ask":
        let opts = [kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String: true] as CFDictionary
        say(AXIsProcessTrustedWithOptions(opts) ? "true" : "false")
    case "window": window(pid)
    case "key" where w.count == 3: key(pid, w[2])
    case "click" where n.count == 2:
        mouse(pid, .mouseMoved, n[0], n[1]); usleep(50_000)
        mouse(pid, .leftMouseDown, n[0], n[1]); usleep(30_000); mouse(pid, .leftMouseUp, n[0], n[1]); say("click")
    case "drag" where n.count == 4:
        mouse(pid, .mouseMoved, n[0], n[1]); usleep(50_000)
        mouse(pid, .leftMouseDown, n[0], n[1]); usleep(60_000)
        for i in 1...20 {
            let t = Double(i) / 20
            mouse(pid, .leftMouseDragged, n[0] + (n[2] - n[0]) * t, n[1] + (n[3] - n[1]) * t); usleep(15_000)
        }
        mouse(pid, .leftMouseUp, n[2], n[3]); say("drag")
    case "front":
        say("was \(NSWorkspace.shared.frontmostApplication?.bundleIdentifier ?? "")")
        NSRunningApplication(processIdentifier: pid)?.activate()
        usleep(400_000)
    case "activate" where w.count == 2:
        NSRunningApplication.runningApplications(withBundleIdentifier: w[1]).first?.activate()
        say("activated \(w[1])")
    case "sleep": usleep(useconds_t((Double(w.count > 1 ? w[1] : "0") ?? 0) * 1000))
    default: say("cannot \(line)")
    }
}
if let p = cursorWas { CGWarpMouseCursorPosition(p) }
try? out.write(toFile: args[2], atomically: true, encoding: .utf8)
