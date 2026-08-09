# remarkable

Handwritten notes off a reMarkable tablet, read by a local vision model and
written out as Markdown. A port of the Python `mhagrelius/remarkable-ocr` onto
this machine's Rust + GTK 4 stack.

## Stack

GTK 4.22 + libadwaita 1.9 via gtk4-rs 0.11 / libadwaita-rs 0.9, Rust edition
2021 (MSRV 1.80). `gio` is a direct dependency purely to raise the API level to
v2_80 — leave it. libsoup 3 is the HTTP client, as in familiar. gdk-pixbuf does
the decoding and cropping; it is already linked into the process and, like cairo
in familiar's document writers, needs no display.

Crate is a lib + bin so `examples/eval` drives the real application rather than
a copy of it.

Reading needs a `llama-server` with a vision projector on port 8080 — the one
llama-tray manages. Reading PDFs needs `pdftoppm` from poppler-utils.

## Commands

- `./test.sh` — fmt check, clippy with `-D warnings`, then `cargo test
  --all-targets`. Add `--headless` to run under Xvfb + a private D-Bus session.
  This is the gate; run it, not bare `cargo test`. Nothing in it talks to a
  server or a tablet.
- **Never run `dbus-run-session` or `xvfb-run -a dbus-run-session` directly** —
  use `isolated-bus [--headless] -- CMD`. A private bus activates its own
  `xdg-document-portal`, which mounts over `/run/user/$UID/doc` and takes the
  login session's portal down with it when the bus exits; every flatpak on the
  machine then fails to launch until it is restarted. `test.sh --headless`
  guards against this internally, but one-off runs of a single test, or of the
  built binary, bypass it.
- `cargo run --release --example eval` — grades real transcriptions against a
  real llama-server. `--filter`, `--samples`, `--write`, `--list`. The suite and
  its scoring live in `src/model/eval/`.
- `./install.sh` — release build, installs under `~/.local`. `./uninstall.sh`
  reverses it.

## Layout

`src/model/` is pure logic with no GTK types. `src/ui/` is widgets, and the only
half that opens a socket or runs a process. The pipeline reads top to bottom:

- `raster.rs` — decode a page (never resampling it), reduce it to one byte per
  row, cut a section out as PNG, and scale that section only if it would cross the
  model's token budget. The only module that touches pixels.
- `sections.rs` — pure arithmetic over that row profile. Where a tall page can
  be cut, and how much neighbouring sections overlap.
- `job.rs` — the state machine. Which section next, what to ask, take the answer.
  Holds no image and no socket, which is what lets the window and the eval drive
  the same pipeline.
- `merge.rs` — finds the seam between two sections' transcripts by bigram
  similarity and splices them. `ANCHOR` and `EXTEND` are tuned; read the comments
  before moving them.
- `device.rs` — the tablet's USB HTTP API. Builds URLs and argument vectors,
  performs no I/O.

## Things that were learned the hard way

- **Thinking must be off.** The server runs the froggeric template with
  `enable_thinking` on. Left on, a full section comes back with empty `content` and
  4,096 tokens of `reasoning_content`; at 8,192 it is still empty. Every request
  sends `chat_template_kwargs: {"enable_thinking": false}`. `reasoning_budget:
  0` does not work — the template emits the block regardless.
- **The Python's OCR correction table is gone on purpose.** `rn`→`m` and `0`→`O`
  were written for a classical OCR engine; against a VLM they only corrupted
  correct text (`P0`→`Po`). See the header of `text.rs`.
- **Do not collapse leading whitespace.** It is list nesting. Only the indent
  common to every line is removed.
- **Uncertainty comes back as a footer, not inline markers.** Qwen3.6 with
  thinking off will not hedge mid-transcription under any wording; it will
  produce a `---UNCERTAIN---` list at the end. `text::split_footer` strips it,
  `text::mark_doubts` turns it into `[unclear: ...]`. The cap of four per
  section is not decoration — uncapped it marked 80+ correct words per run.
  A verification pass and a capped thinking budget were both tried and both
  failed; see DESIGN.md before retrying either.
- **Nothing is resampled unless it has to be.** `Page::decode` keeps every
  pixel; `raster::section_png` scales a section down only if it would cross
  `MAX_SECTION_PIXELS`, which is derived from llama-server's measured image-token
  cap. Do not reintroduce a fixed read width — the old 1,288 cost acronym
  accuracy, and anything above the cap gets resampled a second time by the
  server, which is worse still.
- **`Layout`'s lengths are ratios of the page width, not pixels.** A section is
  `max_section_aspect` times as tall as the page is wide; a gap is
  `min_gap_ratio`. Absolute pixel counts have to be found and changed together
  whenever resolution moves, and one was missed once already.
- **No example in the prompt may come from the sample notebooks.** An earlier
  draft used the real diagram labels, `AlignTech` and `DMS` — the last two added
  because the eval flagged them missed — which turned red checks green while
  changing nothing. `eval::suite::the_prompt_gives_away_no_anchor` fails the
  build on any recurrence. Invent examples.

## Conventions

- Use the `developing-gtk-apps` and `designing-gnome-ui` skills for widget,
  threading, and HIG decisions rather than deriving them again.
- Edit files with the Edit tool. Do not rewrite Rust sources through
  `python3 - <<PY` heredocs or `sed -i`.
- The sibling apps (brain, planner, stickies, familiar, magpie) share this
  layout and these scripts; a pattern established in one is the pattern here.
