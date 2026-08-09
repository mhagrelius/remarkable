# Design

Why this is shaped the way it is. Written against the three sample notebooks in
`src/model/eval/suite.rs`, which are what every number here was measured on.

## The problem the app actually has

A reMarkable exports a **notebook**, not a page. One PNG, however many pages you
wrote. The samples are 1,620 pixels wide and 2,160, 9,176 and 17,002 tall.

So the pipeline is: cut the page into sections, read each, stitch them back.
Everything below follows from that.

**How much that is worth is an open question, and less than this document
originally claimed.** The claim here used to be that one image in one request
returns a summary of the first fifth of the page. That was asserted, never
measured, and is false for this model: a naive one-shot on the 27-megapixel
notebook reaches the last line and scores 34/35 anchors against the pipeline's
33/35. Measured head to head on hand-checked regions:

| region | sectioned pipeline | naive one-shot |
|---|---|---|
| meeting notes, mid-document | **1.46%** CER | 1.88% CER |
| notebook, last page | 2.02% CER | **1.01%** CER |
| anchor recall, 9,176 px doc | 38/39 | 38/39 |
| anchor recall, 17,002 px doc | 33/35 | **34/35** |

A wash. What *is* measured to help is the prompt: the same single section, tuned
prompt versus a bare "transcribe this", is 1.46% against 3.13% CER.

The likely reason splitting buys so little is in *Resolution* below — llama-server
caps every image at ~4,000 tokens whatever its size, so the 27-megapixel page is
crushed to roughly 625 pixels wide before the model sees it, and reads anyway.
Eleven sections spend eleven times the tokens on visual detail the model apparently
did not need at this size. Splitting should still matter for a document long
enough to exhaust the output token limit in one response, which none of the
three samples is. Worth revisiting before adding anything to it.

## Where to cut

Two constraints, in `model/sections.rs`:

- **A ceiling on section height**, as a multiple of the page's width rather than a
  pixel count — about one page of writing, which is the shape the prompt was
  written for. See *Resolution* below for why it is a ratio.
- **Cuts land in whitespace.** A line of handwriting sliced through its
  x-height is not recoverable by either neighbour; both see half a word and
  both guess.

The splitter works on a **row-luminance profile**: one byte per image row, the
mean brightness of that row. That is the whole seam between pixels and
arithmetic. `sections.rs` never sees an image, so its tests run with no display
and no decoder, and `raster.rs` is the only module that knows what a pixel is.

Sections overlap by 18% of the page, shared across the boundaries. Overlapping
costs a re-read of a few lines. Not overlapping costs the lines at every seam.

## Where to join

Two sections' transcripts of the same overlapping strip are rarely the same string.
The model sees each copy with a different amount of surrounding page and spells
the uncertain words differently. So the seam is found by similarity, not
equality: `model/merge.rs` scores lines by Dice coefficient over character
bigrams and looks for the longest *run* of consecutive agreeing lines near the
ends.

Two thresholds, and the gap between them was a bug found by a test:

- `ANCHOR` (0.80) to **start** a run. Strict, because `- line 1` and `- line 2`
  score 0.75 and anchoring on those moves the seam and silently deletes a
  paragraph. Two adjacent list items differing by a digit is the exact shape of
  the failure.
- `EXTEND` (0.62) to **continue** one. Once three lines agree, the fourth is in
  a context that vouches for it and one mistranscribed word should not end the
  seam early.

Finding no seam is not an error — a boundary can fall in a genuine gap — and the
halves are concatenated with a blank line between them.

## Thinking has to be off

The llama-server on this machine runs the froggeric template with
`enable_thinking` on, which is right for familiar and wrong here. Measured:

| Request | Result |
|---|---|
| default (thinking on), `max_tokens: 4096` | empty `content`, 4,096 tokens of `reasoning_content` |
| thinking on, `max_tokens: 8192` | empty `content`, 8,192 tokens of reasoning |
| `reasoning_budget: 0` | reasoning still emitted; the budget only caps it |
| `chat_template_kwargs: {enable_thinking: false}` | 34 tokens, correct answer |

The first row is what the first eval run scored zero on. Asked to transcribe a
page, the model reasons about every line before writing any of it, and on a full
section it never gets to the answer. There is nothing to reason about — the answer
is what the page says.

Turned off per request rather than by relaunching the server, because everything
else on this machine wants it on.

## No post-hoc correction

The Python this was ported from applied a table of OCR corrections: `rn`→`m`,
`0`→`O` before a letter, `0`→`o` after one. Those are written for a classical
OCR engine that confuses glyph shapes. A vision-language model does not make
those mistakes, and on these notebooks the rules only ever fired on text that
was already right — `P0` became `Po`, `0x1F` became `Ox1F`.

Guessing at the model's output after the fact cannot beat asking the model to
mark its own uncertainty. The table is gone.

The Python also collapsed runs of two or more spaces anywhere in a line. Leading
whitespace is list nesting, and that rule flattened every sub-bullet on the
page. Here only the indent **common to every line** is removed — which drops a
uniform shift the model added, four spaces of which would otherwise render as a
code block, while leaving the relative nesting underneath alone.

## The pure/impure split

`model/job.rs` holds the pipeline as a state machine: which section next, what to
ask about it, take the answer back. It holds no image, opens no socket and links
no GTK.

That is what lets `ui/runner.rs` and `examples/eval` drive the *same* pipeline —
one on the GLib main loop updating a text view, the other in a plain loop
printing a score — without either reimplementing the order of operations. The
eval is grading the shipping code, which is the only kind of eval worth having.

## Not streamed

A section is requested whole. There is no use for half a transcription, the unit of
progress the window reports is the section rather than the token, and the
alternative costs an SSE parser plus partial-response state to unwind when a
section fails. A twelve-section notebook gives twelve progress updates over half a
minute, which is enough to see it working.

A section that fails does not end the run. Losing eleven good sections to one hiccup
is the wrong trade; the lapse is recorded, and the transcript is saved with the
gap.

## Getting it off the tablet

The tablet in developer mode serves its own library over HTTP on the USB
network, and `GET /download/{id}/placeholder` returns a PDF **the tablet
rendered itself**.

The alternative is copying `~/.local/share/remarkable/xochitl` over SSH and
parsing the `.rm` v6 line format — a stroke-geometry parser and a renderer,
thousands of lines, versioned against firmware — to arrive at a worse picture
than the device hands over for free. Not done.

SSH still earns a place, because that interface answers only on the USB network.
Forwarding a local port to port 80 on the device reaches the same endpoints from
anywhere the tablet is reachable, which is one `ssh -L` and no new protocol.

## Uncertainty is a footer, not an inline marker

A misreading shaped like a real word is the worst output this can produce: it is
fluent, confident and invisible to a proofreader. The transcript therefore marks
what the model could not make out — `[unclear: vein]` where the page says
"view".

The model does not write those markers. It cannot: asked to hedge inline,
Qwen3.6 with thinking disabled marks **nothing at all**, and that held across
every wording tried — a short rule, a long one with an explicit expectation, a
cost argument, a per-token-class classification rule, and a closing checklist.
Its transcription prior is to emit clean text and it will not interrupt itself.
Two other mechanisms failed the same way:

- **A second verification pass**, shown the image and the transcript and asked
  only to check the acronyms, confirms its own errors — `OK TI` for a page that
  says `T1`, and `FIX X -> X` for words it had read correctly. A model cannot
  see its way out of a misreading it has already committed to.
- **Thinking, capped.** With `reasoning_budget: 200` a single section in isolation
  reads *better* — two of three known-ambiguous tokens come out right. In the
  pipeline it collapses: five of six sections hit the token ceiling, recall falls
  to 26%, and one document takes nine minutes. The isolated result was
  misleading because the real prompt is longer and the reasoning scales with it.

What does work is asking for a **list at the end**. Format compliance is a
different capability from mid-flow self-doubt, and it is one this model has: it
produces a `---UNCERTAIN---` section reliably, and the words in it are the hard
ones. `model::text::split_footer` takes the section off and
`model::text::mark_doubts` turns it into the inline markers, so nothing
downstream — the merge, the writer, the window, the eval's count — knows the
footer existed.

Two constraints on that list were learned by measuring:

- **It must be capped.** Uncapped, the model listed every technical term it saw,
  including ones it had read perfectly: 71 to 89 marks per run, `rustfmt` and
  `Cargo.toml` among them. Four per section, described as *the four you are
  least sure of*, and with "unfamiliar is not the same as illegible" said
  explicitly, brings it to a useful handful — and to zero on the page of clear
  printing, which is the right answer for that page.
- **The eval had to stop punishing it.** A marker sits *inside* a phrase, so
  `use [unclear: rustfmt] for formatting` wedges "unclear" into the middle of an
  anchor and breaks the match. Recall appeared to drop three points until
  `expect::flatten` was taught to ignore the marker. A word read correctly and
  then flagged has still been read correctly; flagging is scored separately, by
  the count.

It is not complete. The model still misses some of its own errors, and a
transcript with marks is not a transcript without them. But the failure mode has
changed from *silent* to *visible*, which is the one that matters.

## Resolution: the server has a budget, and it is the only limit that matters

An early version scaled every page to 1,288 pixels wide, then to the
reMarkable's native 1,620. Both were the wrong shape of answer, because both
were a *width* — a number about one particular tablet — when the real constraint
belongs to the model.

llama-server caps the image and silently resamples anything over its budget.
Sending one section at a range of sizes and reading `usage.prompt_tokens` back:

| sent        |    MP | prompt_tokens |
|-------------|-------|---------------|
| 1288 × 1824 |   2.3 |         2,304 |
| 1620 × 2294 |   3.7 |         3,696 |
| 2048 × 2900 |   5.9 |     **4,052** |
| 2560 × 3625 |   9.3 |     **4,052** |
| 3240 × 4588 |  14.9 |     **4,052** |

The plateau is the cap, and it explains every resolution result this app ever
had. 1,288 read acronyms worse because it genuinely sent less detail. 2,048 and
above read them worse because the server downscaled them *after* they had
already been scaled here — a lossy round trip, and an invisible one. The
tablet's native 1,620 won only by happening to land just under the cap. It was
right by luck.

So the rule is now the one the constraint actually implies:

- `Page::decode` resamples **nothing**, whatever arrives. The splitter also gets
  the finest row profile available for placing its cuts.
- `raster::section_png` scales a section down only if it would cross
  `MAX_SECTION_PIXELS`, and then exactly once, with `InterpType::Hyper` — bilinear
  drops thin strokes between sample points, which on handwriting is the
  ascender that distinguishes a `1` from an `l`.
- Nothing is ever scaled up.

A reMarkable export therefore reaches the model untouched: a 1,620-wide page
gives 1,620 × 2,267 sections, 3.67 MP, 3,624 image tokens against a cap near
4,038. Verified by measuring a real section with the real prompt — 4,714 prompt
tokens, of which 1,090 are the instruction. A 4,000-pixel scan is over budget
and gets scaled once, here, rather than twice.

`Layout` follows from the same thinking. Its lengths are ratios of the page
width — a section is 1.4 pages tall, a gap is a seventy-second of the width —
because the same notebook scanned at twice the resolution is the same notebook.
Absolute pixel counts have to be found and changed together every time the
resolution moves, and one of them was missed the first time this changed:
`min_gap` stayed at its old value, quietly shrinking what counted as a paragraph
break until a line went missing.

## The prompt may not contain the answers

Every example in `prompts/transcribe.md` is invented. Not for tidiness: a phrase
written into the instruction is a phrase the model can produce without having
read it off the page, and recall goes up while accuracy does not.

This was not hypothetical. The prompt's diagram example was
`[Diagram: message → rules based evaluation → LLM based evaluation → sql]`,
which is three meeting-diagrams anchors verbatim. Worse, `AlignTech` and `DMS`
were added to the heading and acronym examples *because the eval had just
reported them missed* — turning two red checks green while changing nothing
about the model's reading.

`eval::suite` now runs every anchor in the suite against the generated prompt
using `expect::recall`, the same matcher the eval scores with, and fails the
build on a hit. A second test points it at the wording it was written for, so
the guard's polarity is known rather than assumed.

It is a floor, not a proof. Two other lines in that draft — `[unclear: Sneha] is
learning the codebase` and ``` `Entra` read as `entire` ``` — fed the model a
name and a product straight off the page without matching a graded anchor, and
no automated check can catch that, because the space of near-misses is
unbounded. Hence: invented examples, always.
