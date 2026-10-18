# Real language-server acceptance

`tests/lsp_real.rs` drives the ordinary `Editor`, its background manager, command
views and edit approvals against installed servers. These tests are opt-in:
`cargo test --locked --all-targets` compiles them and reports eight ignored tests in this suite.
The default suite and `fake_server.py` require no real servers or downloads.

`csharp_generated_and_decompiled_navigation` additionally exercises actual
`StringBuilder` class/member definitions, navigation from decompiled source to
`ISerializable`, and a custom source generator's class and method. Its
`csharp-metadata` fixture references Roslyn from the installed .NET SDK and builds
the generator in the temporary fixture before server import, without third-party
NuGet packages. It checks returned declaration lines, read-only buffers, jump-back
history and unchanged fixture bytes. Run it with the same C# setup below:

```sh
cargo test --locked --test lsp_real csharp_generated_and_decompiled_navigation -- --ignored --nocapture
```

## Provisioning

[servers.json](servers.json) records the exact Linux x64 distributions used for
acceptance. Download/extract servers outside the repository and verify their
checksums before running them. Kotlin, JDT LS and Gradle hashes were checked
against their published SHA-256 files; the .NET hash comes from Microsoft's
[release metadata](https://builds.dotnet.microsoft.com/dotnet/release-metadata/10.0/releases.json).
The csharp-ls hash identifies the NuGet package installed by `dotnet tool`.
The editor can also provision these pinned servers automatically; see the managed
installation workflow below. JDK/.NET SDK and project toolchains remain prerequisites.

- [Official Kotlin release](https://github.com/Kotlin/kotlin-lsp/releases/tag/kotlin-lsp/v263.6379.0):
  use `bin/intellij-server --stdio`. This archive bundles its JBR 25 runtime.
  `--system-path` selects disposable caches; `IJ_JAVA_OPTIONS=-Duser.home=...`
  isolates server preferences where honored. The tested native launcher/importer
  still writes Maven artifacts under the OS home even with `JAVA_TOOL_OPTIONS`
  overrides; inspect its import log and provide a writable Maven cache. Leave
  telemetry at its default `none`.
- [JDT LS](https://github.com/eclipse-jdtls/eclipse.jdt.ls): use the archive's
  `bin/jdtls` with a Java 21+ runtime. The editor supplies a private `-data`
  directory; do not supply another. `JAVA_TOOL_OPTIONS=-Duser.home=...` can
  isolate Maven dependencies. The source fixtures target Java 21.
- [.NET SDK](https://dotnet.microsoft.com/download/dotnet/10.0) and
  [csharp-ls 0.28.0](https://www.nuget.org/packages/csharp-ls/0.28.0): set
  `DOTNET_ROOT`/`PATH` to the extracted SDK, then run
  `dotnet tool install csharp-ls --version 0.28.0 --tool-path /your/test/tools`.
  `DOTNET_CLI_HOME` and `NUGET_PACKAGES` can isolate caches; set
  `DOTNET_CLI_TELEMETRY_OPTOUT=1`. Fixtures target `net10.0`.
- Gradle fixtures pin 9.1.0 and its checksum in wrapper properties. The servers'
  tooling APIs read these properties directly; no wrapper JAR or shell launcher
  is needed for import. Set `GRADLE_USER_HOME` to disposable caches and install
  JDK 21 for the explicit project toolchain. Import may download the distribution,
  Kotlin plugin and dependencies. Each fixture has an app and a library module.

Runtime JDK, project toolchain and compilation target are separate. Do not change
the fixtures' SDK targets to hide an import failure. Maven/Gradle/SDK caches can
outlive a test; distinguish first-download runs from cached dependency runs.

## Running

To exercise the editor's installer with real downloads, use a fresh absolute data
directory and leave command overrides unset:

```sh
EDITOR_PRO_LSP_INSTALL_DIR=/your/test/managed-servers \
  cargo test --locked --test lsp_real kotlin_maven -- --ignored --nocapture --test-threads=1
```

Repeat with `java_maven` and `csharp_project`, providing JDK 21+ and .NET SDK 10+
respectively. Remove server executables from the test PATH to exercise installation;
existing PATH servers retain precedence. The directory is opt-in test configuration, not a production
environment override. The first run downloads/checks/extracts the actual server;
subsequent runs reuse it. Reports include installation failures. Kotlin bundles
its own runtime. A managed JDT launch uses Java directly with private configuration
and workspace directories. Managed C# uses an isolated tool install from the
verified NuGet package. Keep these network-dependent tests separate from the
ordinary local HTTP/archive fixture tests.
Managed fixture waits allow 900 seconds for installation plus initial import;
ordinary existing-server fixture waits retain their 180-second limit.

Set only the override needed for the language under test. For example:

```sh
export EDITOR_PRO_KOTLIN_LSP='["/your/kotlin/bin/intellij-server","--stdio","--system-path=/your/test/kotlin-cache"]'
export EDITOR_PRO_JAVA_LSP='["/your/jdtls/bin/jdtls"]'
export EDITOR_PRO_CSHARP_LSP='["/your/test/tools/csharp-ls"]'
export EDITOR_PRO_LSP_REPORT_DIR=/your/test/reports
cargo test --locked --test lsp_real -- --ignored --nocapture --test-threads=1
```

Use a filter such as `kotlin_maven`, `java_` or `csharp_` before `--` to run a
subset. Run serially so Kotlin's selected cache directory has one owner and cold
indexing does not compete with other servers for CPU/memory. Unset
`EDITOR_PRO_LSP_ROOT`: these tests check actual project discovery. Supply normal
network access for initial dependency imports. No test accesses a user's project;
every fixture is copied to a temporary directory and removed after server teardown.

All fixtures contain an intentional type error and Unicode text. They are not
expected to compile until that error is fixed in memory. Tests wait for actual
diagnostics, exercise hover, definition jumps, references, document symbols,
signature help, a quick-fix preview, completion resolution/application, unsaved
diagnostic changes, formatting, multi-file rename, cancel/apply/undo, dirty quit
protection and restart/resynchronization. Original source/build-file bytes must
remain unchanged on disk. An unavailable server, timeout or failed feature fails
the selected test; it is never converted to a skip or a successful empty result.
Completion also exercises actual typing in Insert mode, a filtered popup, Tab
acceptance including command-provided edits, and undo back to the original
fixture. Completions from either list must apply directly for all three languages;
the reports record zero previews, and a preview fails the fixture.
This uses the same installed server as the explicit completion checks.
M6 also exercises workspace-symbol navigation/lazy resolution and incoming/outgoing
call hierarchy with location-history restoration. If a server does not advertise
one of these optional capabilities, the test verifies the explicit unsupported
response and records it separately. Advertised features must return useful fixture
results and navigate correctly; an empty result does not pass those checks.

Reports include the fixture fingerprint, command, platform, negotiated
capabilities, bounded server logs, phase, per-check results and timings. The Kotlin Gradle fixture also requires
observed Starting, Indexing and Ready states, verifies troubleshooting links, and
records up to 512 status transitions plus maximum observed editor tick duration.
`kotlin_gradle_import_failure_and_recovery` injects a `GradleException` into a
fresh temporary project, requires visible Degraded state from the actual server,
restores the build file, then verifies explicit restart with unsaved source and
fresh diagnostics. Its separate report retains the failure message and timings. They are
also written on assertion failure. Results are headless editor evidence, not
physical-terminal inspection. Initialization/readiness waits cap at 180 seconds;
feature requests retain the editor's normal 15-second deadline. A first import
can time out an early request; preserve that failure before retrying with caches.

JDT indexes remain temporary on each restart. The restart measurement therefore
tests resynchronization with cached dependencies, not persistent JDT index reuse.
These small JVM/.NET fixtures do not establish Android, KMP, mixed Kotlin/Java,
Razor, older framework targeting, macOS, or large production-project support.
Real crash injection and server memory profiling are separate manual checks;
the deterministic suite covers crash/restart races with a protocol double.

The earlier standalone Kotlin protocol experiment remains in `../kotlin-lsp/`.
It checks a narrower Maven workflow, including clean shutdown/exit, independently
of the Rust client. Its reports must not be substituted for these editor tests.

## Startup latency comparison

`tests/lsp_startup_real.rs` adds four opt-in benchmarks for Kotlin Gradle, Java
Maven/Gradle and a C# solution. It uses the production manager and installed
servers, without automatic installation. Supply the same argv overrides or
`EDITOR_PRO_LSP_INSTALL_DIR` as above, with required SDKs on PATH:

```sh
EDITOR_PRO_LSP_REPORT_DIR=/tmp/editor-pro-startup-reports \
  cargo test --locked --test lsp_startup_real -- --ignored --nocapture --test-threads=1
```

Each test alternates lazy/warm/warm/lazy runs with fresh project copies and
processes. Both modes simulate ten seconds of browsing before opening the source;
`EDITOR_PRO_LSP_BROWSE_MS` changes this lead time. Lazy mode starts the manager
on file open, reproducing the previous startup behavior. Warm mode queues project
preparation immediately, then sends the same document snapshot on open. A request
counts only when definition resolves to the expected other source file. Empty or
failed responses are recorded and retried, retaining normal request deadlines;
the overall wait caps at four minutes after opening. Reports include latency from
both launch and file open, initialization, first observed nonempty diagnostics,
phase logs, retries, fixture fingerprint and session epochs (one required).

Dependency/SDK caches are shared and not cleared; fresh processes/projects are
not a measurement of empty-cache downloads. Alternate order exposes some cache
effects but two samples per mode are insufficient for production performance
claims. Do not compare initialize responses alone: Gradle/Maven/.NET import,
indexing or document-triggered work can continue afterward. Keep failures in the
report and distinguish additional cold-download runs from cached comparisons.
