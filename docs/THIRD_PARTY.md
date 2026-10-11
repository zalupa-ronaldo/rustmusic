# Third-party components

RustMusic's own source code is MIT licensed. Bundled helper executables and
shared libraries keep their original licenses. They are separate programs:
RustMusic controls mpv over JSON IPC and invokes yt-dlp and Deno as subprocesses.

- mpv: GPL-2.0-or-later (Homebrew build; inspect the exact formula configuration).
- FFmpeg and codec libraries: the bundled Homebrew configuration includes GPL
  components; do not describe the entire application distribution as MIT-only.
- yt-dlp standalone executable: Unlicense source with bundled third-party code;
  standalone distributions have additional licensing conditions described in
  the upstream LICENSE, including GPL-3.0-or-later components.
- Deno: MIT and third-party notices from its upstream distribution.
- Rust crates: individually listed in `dependencies.json`, with available
  license texts in `RustMusic.app/Contents/Resources/licenses/rust`.

The release includes `third-party-sources.tar.gz`: corresponding upstream
sources, installed Homebrew formula recipes and receipts, configuration metadata,
and formula patches for bundled native libraries. Build recipes specify the
original Homebrew build options. Use the matching compiler and build-time
prerequisites specified by those recipes to rebuild; binaries are relocated
with `install_name_tool`, then ad-hoc signed by `scripts/package-macos.py`.

No proprietary Apple system frameworks are redistributed. Source archives and
installed license texts are packaged alongside the application; source retrieval
or checksum errors fail packaging instead of silently omitting required files.
