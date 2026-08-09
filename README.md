# Remarkable

Handwritten notes off a reMarkable tablet, turned into Markdown by a model
running on your own machine.

Drop a notebook export on the window. You get back headings, nested lists,
emphasis and section rules — text you can paste into a vault, not a wall of
run-on words. Nothing is uploaded, and there is no subscription.

```
## Current Needs
1/27/26

- F & B
  - Sneha, Anisha under maxim/miles
  - Toby in Feb
- NFP Applied Epic
- Greystar SoW
- GMR (large project) — not on my radar

---

## AlignTech
```

## Why it is not just one prompt

A reMarkable exports a whole notebook as **one** image, not one per page. The
samples this was built against are 2,160, 9,176 and 17,002 pixels tall. Handing
27 megapixels to a vision model in one piece gets you a summary of the page
rather than a transcription of it — the encoder has a fixed token budget and
spends it wherever it likes.

So a page is cut into sections, at gaps in the writing rather than at fixed
intervals, and never through a line of text. Adjacent sections overlap, each is
told where it sits and what the section above it read, and the transcripts are
stitched back together by finding the seam. A seventeen-thousand-pixel notebook
becomes eleven requests and comes back whole.

## Requirements

- A `llama-server` with a vision projector, on `http://127.0.0.1:8080`. Built
  and tested against Qwen3.6-27B with `mmproj-F16`.
- `poppler-utils`, for reading PDFs.
- GTK 4.22, libadwaita 1.9, libsoup 3.

## Install

```bash
./install.sh          # builds release, installs under ~/.local
./uninstall.sh
```

## Use

Open a PNG, a JPEG or a PDF — from the button, by dropping it on the window, or
from your file manager. The transcript fills in section by section as it is
read; a long notebook takes a minute or two and shows you where it is. When it
finishes you can edit the text before saving it, because a model reading
handwriting is wrong sometimes and you are the person who can tell.

Save as Markdown (with YAML frontmatter, which [brain](https://github.com/mhagrelius/brain)
reads), JSON, or plain text.

| Shortcut | Does |
|---|---|
| <kbd>Ctrl</kbd>+<kbd>O</kbd> | Open a file |
| <kbd>Ctrl</kbd>+<kbd>S</kbd> | Save the transcript |
| <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>C</kbd> | Copy the transcript |
| <kbd>Esc</kbd> | Stop reading |

## Straight off the tablet

**Main Menu → Open From reMarkable** lists what is on the device and fetches
one, skipping the web app entirely.

It needs the tablet plugged in over USB with **Settings → Storage → USB web
interface** turned on. The tablet then serves its own library at
`http://10.11.99.1`, and `download/{id}/placeholder` gives you a PDF the tablet
rendered itself — its strokes, its layout, better than anything this could
reconstruct from the raw `.rm` files.

That interface only answers over the USB network. To reach it from elsewhere,
forward a port to the device over SSH:

```bash
ssh -N -L 127.0.0.1:8081:127.0.0.1:80 root@<tablet>
```

(The root password is under **Settings → Help → About → Copyrights and
licenses**, at the bottom of the GPLv3 notice.) `model::device::Source::tunnel`
points the same endpoints at the forwarded port.

## How good is it

`cargo run --release --example eval` grades the pipeline against the three
sample notebooks, scoring both whether the Markdown is well-formed and whether
the words on the page came back. On Qwen3.6-27B:

**Word accuracy: about 98%.** Measured by hand-transcribing 321 words across
four samples spanning the difficulty range, and diffing:

| page | words | words read wrong |
|---|---|---|
| neat printing | 100 | 0 |
| tidy cursive | 46 | 0 |
| messy cursive | 175 | 7 |

Every one of those seven is a proper noun, an acronym or a one-syllable
function word — `DMS` read as `PMS`, `T1` as `TI`, `Tamera` as `Tamara`,
`view` as `vein`. Not one word of running prose came back wrong on any sample.
Counting capitalisation, arrow direction and where a compound gets its space,
it is 94.7%.

The suite also tracks anchor recall — ~100 phrases spread through the three
notebooks, which catch a skipped section or a swallowed paragraph rather than
individual words:

| Case | Recall | Faults | Flagged | Sections | Time |
|---|---|---|---|---|---|
| rust-book (2,160px) | 100% | 0 | 0 | 1 | 2s |
| meeting-diagrams (9,176px) | 97% | 0 | 16 | 6 | 32s |
| notebook-long (17,002px) | 94% | 0 | 22 | 11 | 64s |

No example in the prompt is taken from these notebooks. A phrase the instruction
contains is a phrase the model can produce without reading the page, so the
suite fails the build if any anchor it grades on appears in the prompt.

## Words it was not sure of

A misreading that looks like a word is worse than an obvious one, because
nobody proofreading catches it. So the transcript marks what the model could not
make out:

```
Filing Status in the mailbox [unclear: vein]
```

That one is wrong — the page says "view" — and the point is that you can see it
is wrong. Names and acronyms are where this matters: a page of cursive gets a
handful of marks, and a page of clear printing gets none.

On the measured samples the marks caught **four of the seven** real
misreadings, and about one mark in three was a genuine error. So they are a
proofreading aid, not a guarantee: they will not find every mistake, and most
of what they point at turns out to be fine.

Asking for these inline does not work. Qwen3.6 with thinking disabled will not
interrupt its own transcription to hedge, under any wording tried. Asked instead
for a short list at the end of each section, it produces one reliably, and
Remarkable turns that list into the markers above. Neither the list nor the
request for it appears in your transcript.

## Licence

GPL-3.0-or-later.
