# Compatibility

Debian stable is the default image family; Alpine 3.24 is the required second family. An Alpine
container inside a Debian VM is interim and is never counted as native Alpine. Linux hosts and
Linux runners only, with the architecture explicit everywhere.

Locally, advertised architectures follow the nested arm64 host. `x64` labels stay unadvertised
until NE2 passes on native x86_64 hardware. Native `smol-alpine…` labels stay unadvertised until
NE1 passes the native Alpine corpus.

The transparent Actions cache v2 adapter is out of scope because evaluating it needs live GitHub.
Normal GitHub cache and artifact endpoints remain. The full Debian-versus-`ubuntu-latest` corpus
and its remedies are written up when F2 lands.

The official runner v2.338.2 targets .NET 8, which went out of support on 2026-11-10. The .NET 10
change is upstream PR #4757 and is still open; the bundled target is recorded in job history.
