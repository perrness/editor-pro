# Language-server feasibility

Investigated and tested 2026-10-14. The Rust editor implements a standard stdio LSP
client for M5. Six opt-in tests now pass against pinned Kotlin/JDT/Roslyn servers
on Linux, separately from the deterministic protocol-double tests. The independent
Kotlin experiment also passes. See README for client setup/commands and
[tools/lsp](../tools/lsp/README.md) for exact distributions and reproduction.

The editor now installs missing servers automatically, using pinned distributions
and checksums. `:LspInstall [kotlin|java|csharp]` retries or starts an explicit
installation; `:LspInstallCancel [LANGUAGE]` cancels it. See README for native
storage paths and the `lsp.auto_install` setting. Kotlin's bundle supplies its
runtime; Java and C# still require installed JDK 21+ and .NET SDK 10+ respectively.
The earlier feasibility observations below describe separately provisioned servers;
the automatic-installation evidence is recorded in VALIDATION.md.

## Selected servers

| Language | Candidate | Requirements and limits | Decision |
| --- | --- | --- | --- |
| Kotlin | Official JetBrains Kotlin LSP | IntelliJ-based; alpha and partially closed-source. JVM Gradle/Maven support, experimental Android; KMP is still described as future work. Standalone releases exist. | 263.6379.0 passed the isolated Maven probe and editor Maven/multi-module Gradle fixtures. |
| Java | Eclipse JDT LS | Java 21+ to run; its documented compiler support spans Java 8–25. Maven, Gradle and standalone Java support. Separate workspace data directory and launch configuration need management. | 1.60.0 passed Maven/multi-module Gradle editor fixtures; project targets and Gradle toolchains were Java 21. |
| C# | `csharp-ls` | Community Roslyn-based server, MIT licensed; current install requires .NET SDK 10+. Can target older frameworks, subject to their SDK/reference assemblies and project support. | 0.28.0 with SDK 10.0.402 passed `.csproj` and multi-project `.sln` editor fixtures targeting `net10.0`. |

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

The pinned runtime has now demonstrated standard stdio interoperability and
essential features on small Maven and two-module Gradle JVM fixtures. These
observations do not establish KMP, Android, large Gradle builds or Java/Kotlin
coordination. Alpha status and partial source availability increase upgrade and
debugging risk; rerun acceptance when changing server versions.

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
The fixture tests Maven only; the separate Rust editor suite exercises Gradle.

## Observed Linux results

The earlier missing-executable blocker is resolved. Servers and SDKs were
provisioned under temporary directories with verified distribution hashes,
isolated caches and explicit argv overrides. Those earlier acceptance runs provisioned
servers separately; real servers remain outside the ordinary test prerequisites.
See [servers.json](../tools/lsp/servers.json).

- Official Kotlin **263.6379.0** uses its bundled **JBR 25.0.4.1+1-b601.61**.
  The standalone probe passed in **63.052 seconds**, including version-2
  diagnostics after an unsaved edit and clean exit. Explicitly closing stdin after
  `exit` was necessary for its transport loop to finish. Eight Python tests cover
  the harness independently, including this EOF behavior.
- JDT LS **1.60.0-202606262232** ran with OpenJDK **25.0.4.1**; the archive reports
  server version `1.60.0-SNAPSHOT`. Signature help must be explicitly enabled
  despite the advertised capability; the client now supplies that setting.
- `csharp-ls` **0.28.0** ran with **.NET SDK 10.0.402** and `net10.0` projects.
  Its null-configuration warning falls back to defaults and did not prevent any
  tested feature. Java/Kotlin fixtures target JVM 21; Gradle **9.1.0** fixtures
  explicitly select the installed JDK 21 toolchain. Maven **3.8.7** is available.

The six editor tests exercise actual project discovery/import, diagnostics,
completion resolution/application, hover, signature help, cross-file definition
and references, document symbols, quick-fix previews, formatting, multi-file
rename, cancellation/undo, dirty quit protection and unsaved restart/resync.
Original source/build files stay byte-for-byte unchanged. Kotlin and C# supplied
version-verified diagnostics; JDT push reports were versionless and remain visibly
unverified. [VALIDATION.md](../VALIDATION.md) records timings and validation commands.

## Remaining validation

The measured fresh-project/restart runs use cached dependencies. A first JDT
Maven import exceeded the normal 15-second references deadline; a cached retry
passed. More representative cold imports, peak memory, larger production builds,
mixed Kotlin/Java navigation and real-server crash injection still need observation.
Protocol doubles cover missing servers, deadlines, crashes and asynchronous races;
they do not replace these real workload checks.

macOS and physical-terminal appearance remain unobserved. Android, KMP, Razor,
older .NET targets and multiple-solution selection are not established by these
fixtures. JDT indexes remain temporary per session. The client still excludes
snippets, virtual documents, watched-file registration and file resource operations.
Use the manual workflow in README on your projects before relying on those
environments; record unsupported and failed behavior separately from passes.
