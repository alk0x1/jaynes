# Reach spike (THROWAWAY)

A throwaway prototype, not part of Jaynes: it shows how to give a process (a script, a browser, an agent) network
to a list of origins only, imposed from outside it, and how to prove from the run's record that nothing else left.
Nothing here is imported by `engine/` or `app/`; on macOS it only reads `engine/python/jaynes_box.py` to write the
box profile Jaynes writes today. Delete it once the step's network is designed.

- `proxy.mjs`: the one door out. Lets through only `host:port` origins of a list (HTTP, CONNECT for HTTPS and
  WebSocket), resolves names itself, refuses any address in a closed range (loopback, private, link-local, metadata)
  unless named as open, and writes one line per request, let through or refused.
- `web.mjs`: a pretend web. `allowed` is the step's origin, `evil` is anywhere else, `host` stands for a service on
  the person's machine. Each counts what reaches it, so leaks are also seen from the far side.
- `probe.mjs`: runs inside a box and tries every way out: through the proxy, a redirect, WebSocket, an allowed name
  that points home, straight by name and by IP, UDP, DNS, the public internet, the person's local service, a
  browser's debugger port, the cloud metadata address.
- `mac.mjs`: the probe and a browser on macOS, open, in the box Jaynes gives a project's commands today
  (`sandbox-exec`, any local port), and in that box opened only to the proxy's port.
- `linux.mjs` + `linux/Dockerfile`: the same in Linux containers: open, forced (the box's only network peer is the
  proxy), and a firewall in the box's own network namespace.

## Run

```sh
node mac.mjs                                                      # macOS, no container
docker build -t jaynes-reach-spike linux && node linux.mjs        # Linux, from macOS with Docker
```

The pretend web uses ports 9100 to 9104 on 127.0.0.1 on macOS, and networks with no way out in Docker. The only
real addresses touched are a DNS lookup of example.com and a TCP connect, with nothing sent, to 1.1.1.1:443, to see
whether a box lets them out. Browsers are a test Chromium (Playwright's headless shell), each with a profile of its
own and a pretend keychain; the person's Chrome is never started. Results land in `runs/` (ignored by git).

## What it showed (macOS 26, Linux in OrbStack 7.2 with Debian 12, Chromium 156 and Chrome for Testing 159)

| | far side reached | how |
|---|---|---|
| proxy flag or `HTTP_PROXY` only | yes | the program can ignore it; Chrome sends loopback straight by default |
| macOS, Jaynes box today (any local port) | yes | any local service, and a browser's debugger port (`/json/version` answered) |
| macOS, Jaynes box opened only to the proxy's port | no | every direct try is `EPERM` |
| Linux, ordinary container | yes | also the Mac's own local services through `host.docker.internal` |
| Linux, internal network whose one peer is the proxy | no | `ENETUNREACH` |
| Linux, firewall in the box's namespace | no for TCP, yes for DNS | Docker's resolver sits on loopback and forwards names out |
| Linux, `bwrap --unshare-net` + a socket file to the proxy | no | needs user namespaces: refused under Docker's default seccomp |

In every box the proxy refused the redirect elsewhere, the WebSocket elsewhere and the allowed name pointing home.
A page cannot tell: a `no-cors` fetch the proxy refused still says it was sent. The proof is the proxy's log plus the
box being closed, not what the program reports.
