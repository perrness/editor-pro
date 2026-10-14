# Language-server feasibility

Investigated 2026-10-14 using upstream documentation. This is a decision record,
not a claim that the Rust editor already speaks LSP. There are no LSP dependencies
or subprocesses in the editor binary. The experiment is in `tools/kotlin-lsp/`.

## Initial choices

| Language | Candidate | Requirements and limits | Decision |
| --- | --- | --- | --- |
| Kotlin | Official JetBrains Kotlin LSP | IntelliJ-based; alpha and partially closed-source. JVM Gradle/Maven support, experimental Android; KMP is still described as future work. Standalone releases exist. | Preferred; must pass the isolated probe and representative-project testing before promising support. |
| Java | Eclipse JDT LS | Java 21+ to run; its documented compiler support spans Java 8–25. Maven, Gradle and standalone Java support. Separate workspace data directory and launch configuration need management. | Preferred, mature starting point. Validate actual project JDK/toolchain requirements separately. |
| C# | `csharp-ls` | Community Roslyn-based server, MIT licensed; current install requires .NET SDK 10+. Can target older frameworks, subject to their SDK/reference assemblies and project support. | Initial candidate; install as a .NET tool and test real `.sln`/`.csproj` workspaces before M5. |

Sources: [Kotlin upstream README](https://github.com/Kotlin/kotlin-lsp/blob/main/README.md),
[JDT LS README](https://github.com/eclipse-jdtls/eclipse.jdt.ls/blob/main/README.md),
[csharp-ls README](https://github.com/razzmatazz/csharp-language-server/blob/master/README.md).

Kotlin advertises completion, diagnostics/quick fixes, rename, formatting,
navigation/hover and call hierarchy. JDT LS advertises these core categories plus
symbols, code actions and hierarchy features;
each protocol capability must still be checked at initialization. `csharp-ls`
documents completion, diagnostics and refactoring, with optional custom URIs for
decompiled/generated definitions. Do not equate an advertised category with
every required method working on every project. In particular, record signature
help, references, workspace symbols and hierarchy behavior explicitly at M5.

For C# we can initially leave custom metadata URIs disabled. Supporting them later
requires `csharp/metadata` and a read-only virtual document, rather than treating
the URI as a local file. No assumption is made that Microsoft's VS Code extension
or C# Dev Kit binaries are an interchangeable generic LSP distribution.
[csharp-ls metadata extension](https://github.com/razzmatazz/csharp-language-server/blob/master/docs/features.md).

## Kotlin: version-specific findings

The latest release page observed is **v263.6379.0, October 3, 2026**. It distributes
standalone archives for Linux and macOS architectures with SHA-256 links. Release
notes describe project-import and diagnostics/performance changes; pin a tested
archive and verify its published checksum instead of tracking a moving download.
[Release v263.6379.0](https://github.com/Kotlin/kotlin-lsp/releases/tag/kotlin-lsp%2Fv263.6379.0).

Since v262.4739.0, the documented launcher is `bin/intellij-server`, the previous
`kotlin-lsp.sh` is deprecated, and the server requires **JDK 25**. The release
notes also describe stdout isolation for stdio framing. These release-specific
instructions supersede older README/Neovim snippets which still mention the
legacy launcher. The current maintained Neovim configuration uses
`intellij-server --stdio`. Check the selected archive's `--help` when reproducing.
[Release history](https://github.com/Kotlin/kotlin-lsp/blob/main/RELEASES.md),
[current Neovim launcher configuration](https://github.com/neovim/nvim-lspconfig/blob/master/lsp/kotlin_lsp.lua).

The inference from these sources is that a standard stdio LSP client is feasible,
but project import, runtime packaging, capability details and cross-language
behavior remain empirical questions. A JVM smoke test says nothing about KMP,
Android, a large multi-module Gradle build or Java/Kotlin coordination. Alpha
status and partial source availability increase upgrade/debugging risk.

## Isolated executable probe

The Python standard-library client copies a tiny Kotlin/Maven project to a
temporary directory and launches the selected standalone command without a
shell. The fixture has two source files, a cross-file call, a standard-library
member and one intentional type error. The type error is a test stimulus: the
checked-in fixture is deliberately not a clean Maven build.

```sh
python3 tools/kotlin-lsp/probe.py \
  --server '/path/to/kotlin-lsp/bin/intellij-server --stdio' \
  --timeout 180 \
  --output /tmp/editor-pro-kotlin.json \
  --stderr /tmp/editor-pro-kotlin.stderr.log
```

Use a JDK 25 runtime for the server. Project dependencies and the Kotlin Maven
plugin may need network access on first import. The fixture targets JVM 21 and
uses Kotlin 2.3.0; the server runtime and compilation target are separate choices.
Download/install the standalone archive using its release instructions. The probe
does not install or update software and never touches a user's source tree.

It checks:

1. Framed JSON-RPC initialization, UTF-16 negotiation, capabilities and server info.
2. `initialized` and versioned `didOpen`; waits for an actual type diagnostic
   instead of sleeping for a presumed indexing interval.
3. Definition from `Main.kt` to `Greeter.kt`, nonempty hover, and completion
   containing `String.length` (exercises dependency resolution).
4. Versioned `didChange`, using the advertised full/incremental synchronization
   kind, followed by removal of the intentional type diagnostic.
5. `didClose`, shutdown/exit and a clean process exit.

The client answers configuration/workspace/progress requests and rejects server
edits. Stderr is captured separately from stdout; framing errors are failures.
Deadlines and process-group cleanup prevent a hung server from hanging the probe.
The report records results, fixture fingerprint, notifications and requests.
Exit status: 0 observed pass, 1 failure, 2 missing prerequisite.

The current probe expects push diagnostics. A pull-only server requires adapting
the experiment before drawing a product conclusion. Older diagnostic versions
are ignored; versionless reports are accepted but explicitly recorded as not
version-verified. This does not prove production stale-response correctness.
The fixture tests Maven only; Gradle validation remains a separate gate.

## Observed result in this environment

- OpenJDK **25.0.4.1** and Maven **3.8.7** are available; JDK 21 is also installed.
- Neither `intellij-server` nor `kotlin-lsp` is installed/on PATH.
- During foundation revalidation, authorized network access worked for Rust
  installation. No standalone Kotlin server archive/launcher is installed, and
  none was downloaded or executed. The precise probe blocker is the missing
  server executable; JDK 25 and Maven are present.
- Running the probe returned **blocked**, exit 2, with reason “Kotlin LSP
  executable is not installed or not on PATH.” No capability or functional result
  was obtained from a real server.
- Seven Python tests pass, including a deliberately labelled protocol test
  double, fragmented messages, server-initiated configuration, stale diagnostics,
  missing executable and timeout cleanup. These validate the harness only.

**Conclusion:** no fundamental incompatibility was identified in upstream
documentation; actual Kotlin runtime feasibility is **unverified**. Keep this
gate open. Do not describe mock-server success as Kotlin support.

## Required real-project follow-up

Run and retain the probe report with the exact archive version/checksum, OS,
architecture, runtime JDK and server log. Then test a representative Gradle JVM
project (including wrapper/toolchain discovery and multi-module import) and a
Maven project with real dependencies. Measure cold/warm indexing and memory;
exercise unsaved changes, definition/references, completion resolution, formatting,
rename/code actions, crash/restart and diagnostics after out-of-order edits.

For Java, provision JDT LS and an isolated workspace data directory; check Gradle,
Maven and mixed Kotlin/Java navigation. For C#, install a pinned `csharp-ls`, load
both solution and project-only examples, restore dependencies, and test multiple
solutions/framework targeting. Record supported, unsupported, failed and untested
methods separately; do not turn missing capability into a silent no-op.
