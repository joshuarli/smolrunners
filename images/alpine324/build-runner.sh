#!/bin/sh
set -eu

outdir=${1:?usage: build-runner.sh OUTDIR CACHE}
cache=${2:?usage: build-runner.sh OUTDIR CACHE}
recipe_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)
repo=$(CDPATH='' cd -- "$recipe_dir/../.." && pwd -P)
tmp="$cache/build-tmp/musl-runner"

fail() { printf 'build-runner: %s\n' "$*" >&2; exit 1; }

fetch_input() {
  name=$1
  path=$(sh "$repo/bootstrap/buildlib-run" -- "$repo/scripts/fetch-inputs.py" \
    --name "$name" --cache "$cache/inputs" | tail -1 | cut -f2)
  [ -s "$path" ] || fail "fetch-inputs produced no $name archive"
  printf '%s' "$path"
}

mkdir -p "$tmp" "$outdir" "$tmp/home" "$tmp/nuget"

printf 'build-runner: fetching pinned SDK and runner source\n'
sdk=$(fetch_input dotnet-sdk-linux-arm64)
src=$(fetch_input runner-src-musl-arm64)

rm -rf "$tmp/sdk" "$tmp/src"
mkdir -p "$tmp/sdk" "$tmp/src"
tar -xzf "$sdk" -C "$tmp/sdk"
tar -xzf "$src" -C "$tmp/src"
srcdir=$(find "$tmp/src" -mindepth 1 -maxdepth 1 -type d | head -1)
[ -n "$srcdir" ] || fail 'runner source archive has no top-level directory'
dotnet="$tmp/sdk/dotnet"
[ -x "$dotnet" ] || fail 'pinned SDK has no dotnet executable'
export HOME="$tmp/home"
export NUGET_PACKAGES="$tmp/nuget"
export DOTNET_CLI_TELEMETRY_OPTOUT=1
export DOTNET_NOLOGO=1
export DOTNET_SKIP_FIRST_TIME_EXPERIENCE=1
export DOTNET_SYSTEM_GLOBALIZATION_INVARIANT=1
sdk_version=$("$dotnet" --version 2>/dev/null)
[ "$sdk_version" = 10.0.404 ] || fail "unexpected SDK version: $sdk_version"

printf 'build-runner: retargeting net10.0 and linux-musl-arm64\n'
find "$srcdir/src" -name '*.csproj' -exec \
  sed -i 's|<TargetFramework>net8.0</TargetFramework>|<TargetFramework>net10.0</TargetFramework>|' {} +
find "$srcdir/src" -name '*.csproj' -exec \
  sed -i 's|<RuntimeIdentifiers>|<RuntimeIdentifiers>linux-musl-arm64;|' {} +
sed -i 's|<TreatWarningsAsErrors>true</TreatWarningsAsErrors>|<TreatWarningsAsErrors>false</TreatWarningsAsErrors>|' \
  "$srcdir/src/Directory.Build.props"
cat > "$srcdir/src/Runner.Sdk/BuildConstants.cs" <<'EOF'
namespace GitHub.Runner.Sdk
{
    public static class BuildConstants
    {
        public static class Source
        {
            public static readonly string CommitHash = "d1eb12d4b115c301201de27d667284e1a80f2ab5";
        }

        public static class RunnerPackage
        {
            public static readonly string PackageName = "linux-musl-arm64";
            public static readonly string Version = "2.338.2";
        }
    }
}
EOF

printf 'build-runner: publishing the musl-arm64 apphosts\n'
for project in Runner.Listener Runner.Worker Runner.PluginHost; do
  "$dotnet" publish "$srcdir/src/$project/$project.csproj" \
    -c Release -r linux-musl-arm64 --self-contained true \
    -p:TreatWarningsAsErrors=false -o "$outdir" > "$tmp/$project.log" 2>&1 || \
    fail "$project publish failed: $(tail -25 "$tmp/$project.log")"
done

for apphost in Runner.Listener Runner.Worker Runner.PluginHost; do
  [ -x "$outdir/$apphost" ] || fail "missing published apphost $apphost"
done
printf 'build-runner: ok (SDK %s)\n' "$sdk_version"
