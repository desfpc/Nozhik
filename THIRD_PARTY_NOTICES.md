# Third-party components in Nozhik

Nozhik's Source-Available license applies to original Nozhik material only.
Third-party components retain their own copyright and license terms. Nothing
in LICENSE limits rights granted independently by those licenses.

Binary distributions produced by `scripts/update-binaries.py` include:

- `LICENSE`: the Nozhik Source-Available License.
- `THIRD_PARTY_NOTICES.md`: this overview.
- `THIRD_PARTY_LICENSES.txt`: target-specific Cargo package versions, declared
  licenses, author/repository information and supplied license/notice texts,
  including nested native library and font notices.
- `THIRD_PARTY_SOURCES.zip`: original registry source packages for MPL components
  and packages that do not ship standalone license texts. These sources retain
  their upstream terms, not the Nozhik license. The archive includes manifests
  and build inputs supplied by the upstream crate. Nozhik does not modify them.

These files are also inside `Nozhik.app/Contents/Resources/Legal/` in the macOS
archive. Preserve them when packaging official releases. Package inventory may
include build-time or otherwise unused dependencies; inclusion does not mean
all listed code is present in the executable. It is generated from the locked
Cargo graph for the requested target, not a universal audit of every platform.

## Components with additional conditions

- `option-ext` (through `directories`): MPL-2.0. Its corresponding source is in
  THIRD_PARTY_SOURCES.zip, available to every recipient with the binary. Changes
  to MPL-covered files must remain under MPL and be supplied in that archive.
  https://www.mozilla.org/MPL/2.0/
- Vendored libgit2 (through `git2` / `libgit2-sys`): GPLv2 with a linking exception
  permitting distribution of combined executables under different terms. The
  wrapper's MIT/Apache declaration does not replace libgit2's own license.
  Its COPYING, AUTHORS and bundled notices are reproduced in the generated text.
  https://github.com/libgit2/libgit2/blob/main/COPYING
- OpenSSL, libssh2, SQLite, ring and other bundled native code have separate
  upstream terms; consult their nested entries in THIRD_PARTY_LICENSES.txt.
- JetBrains Mono Regular: Copyright 2020 The JetBrains Mono Project Authors;
  SIL Open Font License 1.1. The original font license is in
  crates/ui/assets/fonts/OFL.txt and reproduced in the generated text.
  https://github.com/JetBrains/JetBrainsMono
- egui default fonts include Ubuntu, Noto Emoji, Hack and emoji-icon-font.
  Their original OFL, Ubuntu Font Licence, MIT and other supplied notices are
  reproduced from the locked `epaint_default_fonts` package.
- Unicode/ICU data and root certificate data retain their Unicode and
  CDLA-Permissive license notices as recorded in the generated inventory.

## Generated PHP catalogs and external tools

PHP catalogs are generated from PHP Reflection, not copied from JetBrains or
Intelephense stubs. Runtime versions, container image digests and extensions are
recorded in crates/language_support/data/catalog-manifest.json. The generator
also retains Reflection documentation where supplied. Review any third-party
documentation added to these catalogs under its originating terms. The PHP
runtime/container images are not distributed with the IDE.

PHP language servers (phpactor/Intelephense), Ollama and AI model weights are
external tools or user downloads, not included in these binary packages. Their
licenses apply separately. A Nozhik purchase does not purchase third-party
services, models or premium language-server features.

## Release maintenance

Regenerate notices and source archives whenever dependencies, features or targets
change. Review licenses of new dependencies and any copied code, assets or data;
automated collection is not a compatibility decision. Additional runtime DLLs
bundled by a Windows GNU cross-build require review of their own license and
source/exception requirements before public distribution. No Windows GNU runtime
compliance claim is made by the Cargo inventory alone.

Files under licenses/ provide standard license texts for registry packages whose
published archives omit standalone license files. For these packages, the build
also supplies the original source package, preserving embedded notices and author
metadata. Apache-2.0.txt is retained for third-party dependencies; it is not the
license for original Nozhik code.
