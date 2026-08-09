//! Stitching the sections of a page back into one transcript.
//!
//! Sections overlap, so the last few lines of one section and the first few of the
//! next are the same handwriting read twice. They are rarely the same *text* —
//! the model sees each copy in a different context, with a different amount of
//! the surrounding page, and spells the uncertain words differently. So the
//! seam cannot be found by comparing strings for equality.
//!
//! What it looks for instead is the longest run of consecutive lines near the
//! seam that are *similar enough*, scored by character-bigram overlap. A run of
//! three agreeing lines is a seam; one agreeing line is a coincidence, and the
//! scoring says so. Finding no seam at all is not an error — it happens when a
//! section boundary falls in a genuine gap in the writing — and the two halves are
//! simply concatenated.

/// How alike two lines must be to *start* a run.
///
/// Strict, because an anchor in the wrong place moves the seam and silently
/// deletes a paragraph. Two list items that differ in one character —
/// `- line 1` and `- line 2`, or two numbered steps — score around 0.75 on
/// bigram overlap, and anchoring on those is exactly the failure this
/// threshold exists to prevent.
const ANCHOR: f32 = 0.80;

/// How alike two lines must be to *continue* a run already anchored.
///
/// Looser than [`ANCHOR`]: once three lines agree, the fourth is being read in
/// a context that says it belongs, and one badly-transcribed word in it should
/// not end the seam early.
const EXTEND: f32 = 0.62;

/// How far into a section's ends to look for the seam.
///
/// The overlap is a fixed fraction of the page, so the seam is always near the
/// ends. Searching further finds spurious matches in a page that repeats
/// itself — meeting notes list the same client name a dozen times.
const SEARCH_DEPTH: usize = 14;

/// Join the transcripts of a page's sections, dropping what the overlap read
/// twice. Sections must be in top-to-bottom order.
pub fn merge(sections: &[String]) -> String {
    let mut merged: Vec<&str> = Vec::new();

    for (position, section) in sections.iter().enumerate() {
        let lines: Vec<&str> = section.lines().collect();

        if position == 0 {
            merged = lines;
            continue;
        }

        let (keep, resume) = seam(&merged, &lines);
        merged.truncate(keep);
        // A seam found mid-paragraph joins directly; one that was not found at
        // all gets a blank line, so two unrelated halves do not run together.
        if resume == 0 && !merged.last().is_some_and(|line| line.trim().is_empty()) {
            merged.push("");
        }
        merged.extend_from_slice(&lines[resume.min(lines.len())..]);
    }

    trim_blank_edges(&merged).join("\n")
}

/// Where the previous section should stop and the next should start.
///
/// Returns `(keep, resume)`: keep `above[..keep]`, then take `below[resume..]`.
/// `(above.len(), 0)` means no seam was found.
fn seam(above: &[&str], below: &[&str]) -> (usize, usize) {
    let tail_from = above.len().saturating_sub(SEARCH_DEPTH);
    let head_to = below.len().min(SEARCH_DEPTH);

    let mut best: Option<(f32, usize, usize)> = None;

    for i in tail_from..above.len() {
        if above[i].trim().is_empty() {
            continue;
        }
        for j in 0..head_to {
            if similarity(above[i], below[j]) < ANCHOR {
                continue;
            }

            // Extend the match downwards through both sections for as long as
            // they keep agreeing. Blank lines extend a run but do not vouch
            // for it.
            let mut run = 0usize;
            let mut agreed = 0.0f32;
            let mut vouched = 0usize;
            while i + run < above.len() && j + run < below.len() {
                let (a, b) = (above[i + run], below[j + run]);
                if a.trim().is_empty() && b.trim().is_empty() {
                    run += 1;
                    continue;
                }
                let score = similarity(a, b);
                if score < EXTEND {
                    break;
                }
                agreed += score;
                vouched += 1;
                run += 1;
            }

            // A long agreeing run beats a good short one: three lines in a row
            // cannot be a coincidence, and one line very often is. Position
            // only breaks ties — weighted below one matched line — but it
            // breaks them the right way, because the overlap is at the end of
            // the section above and the start of the one below. A page that says
            // the same thing twice gets the later occurrence.
            let late_above = i as f32 / above.len().max(1) as f32;
            let early_below = 1.0 - j as f32 / below.len().max(1) as f32;
            let score = vouched as f32 * 2.0 + agreed + late_above + early_below * 0.5;
            if best.map_or(true, |(previous, _, _)| score > previous) {
                best = Some((score, i + run, j + run));
            }
        }
    }

    best.map_or((above.len(), 0), |(_, keep, resume)| (keep, resume))
}

/// How alike two lines are, from 0 to 1.
///
/// Dice's coefficient over character bigrams: the shared bigrams, doubled, over
/// the total. Chosen over edit distance because it is unbothered by a word
/// moving and because it is linear — the seam search runs it a few hundred
/// times per boundary.
pub fn similarity(a: &str, b: &str) -> f32 {
    let (a, b) = (normalise(a), normalise(b));

    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    if a == b {
        return 1.0;
    }

    let left = bigrams(&a);
    let mut right = bigrams(&b);
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }

    let total = left.len() + right.len();
    let mut shared = 0usize;
    for bigram in left {
        // Removed once matched, so "aaa" and "aaaaaa" do not score as one.
        if let Some(at) = right.iter().position(|other| *other == bigram) {
            right.swap_remove(at);
            shared += 1;
        }
    }

    2.0 * shared as f32 / total as f32
}

/// Compare on the words, not the decoration. One section may bullet a line the
/// next section renders plain, and that should not stop them being the same line.
fn normalise(line: &str) -> String {
    line.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn bigrams(text: &str) -> Vec<(char, char)> {
    let characters: Vec<char> = text.chars().collect();
    characters
        .windows(2)
        .map(|pair| (pair[0], pair[1]))
        .collect()
}

fn trim_blank_edges<'a>(lines: &'a [&'a str]) -> &'a [&'a str] {
    let start = lines
        .iter()
        .position(|line| !line.trim().is_empty())
        .unwrap_or(lines.len());
    let end = lines
        .iter()
        .rposition(|line| !line.trim().is_empty())
        .map_or(start, |at| at + 1);
    &lines[start..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_section_comes_through_unchanged() {
        let text = "- first\n- second".to_string();
        assert_eq!(merge(std::slice::from_ref(&text)), text);
    }

    #[test]
    fn nothing_at_all_merges_to_nothing() {
        assert_eq!(merge(&[]), "");
    }

    #[test]
    fn an_exact_overlap_is_read_once() {
        let above = "- one\n- two\n- three\n- four".to_string();
        let below = "- three\n- four\n- five\n- six".to_string();
        assert_eq!(
            merge(&[above, below]),
            "- one\n- two\n- three\n- four\n- five\n- six"
        );
    }

    #[test]
    fn an_overlap_the_two_sections_spell_differently_is_still_read_once() {
        // The seam as it actually arrives: the same handwriting, transcribed
        // twice, agreeing on the shape of the words and not on every letter.
        let above = "- stay as far away from servers\n- coded apps they like".to_string();
        let below = "- coded apps they liked\n- Notifications".to_string();
        let merged = merge(&[above, below]);
        assert_eq!(
            merged.lines().filter(|l| l.contains("coded apps")).count(),
            1
        );
        assert!(merged.contains("stay as far away"));
        assert!(merged.contains("Notifications"));
    }

    #[test]
    fn a_long_agreeing_run_wins_over_a_single_coincidence() {
        // "- Eaton" appears early in the second section by chance. The real seam
        // is the three lines that follow it.
        let above = "- Eaton\n- AMH\n- WealthSpire\n- TopBuild".to_string();
        let below = "- AMH\n- WealthSpire\n- TopBuild\n- Huntington".to_string();
        assert_eq!(
            merge(&[above, below]),
            "- Eaton\n- AMH\n- WealthSpire\n- TopBuild\n- Huntington"
        );
    }

    #[test]
    fn sections_with_no_seam_are_both_kept() {
        let above = "# Current Needs\n\n- F & B".to_string();
        let below = "# AlignTech\n\n- Presales".to_string();
        let merged = merge(&[above, below]);
        assert!(merged.contains("Current Needs"));
        assert!(merged.contains("AlignTech"));
        assert!(merged.contains("F & B"));
        assert!(merged.contains("Presales"));
    }

    #[test]
    fn three_sections_stitch_in_order() {
        let sections = [
            "alpha\nbravo\ncharlie".to_string(),
            "bravo\ncharlie\ndelta\necho".to_string(),
            "delta\necho\nfoxtrot".to_string(),
        ];
        assert_eq!(
            merge(&sections),
            "alpha\nbravo\ncharlie\ndelta\necho\nfoxtrot"
        );
    }

    #[test]
    fn a_section_that_read_nothing_does_not_swallow_its_neighbours() {
        let sections = [
            "alpha\nbravo".to_string(),
            String::new(),
            "charlie\ndelta".to_string(),
        ];
        let merged = merge(&sections);
        assert!(merged.contains("alpha"));
        assert!(merged.contains("delta"));
    }

    #[test]
    fn similarity_knows_the_same_line_from_a_different_one() {
        assert_eq!(similarity("", ""), 1.0);
        assert_eq!(similarity("hello", ""), 0.0);
        assert_eq!(similarity("hello", "hello"), 1.0);
        // Bullet added by one section and not the other.
        assert!(similarity("- Workday not yet", "Workday not yet") > 0.95);
        // One letter off, as OCR of handwriting goes.
        assert!(similarity("auditing is second", "auditing is secord") > ANCHOR);
        // Two different bullets in the same list.
        assert!(similarity("- Interns?", "- Workday not yet") < EXTEND);
        // Two list items a digit apart: the case that must not anchor a seam.
        assert!(similarity("- line 1", "- line 2") < ANCHOR);
    }

    #[test]
    fn repeated_characters_do_not_inflate_a_match() {
        assert!(similarity("aaa", "aaaaaaaaaaaa") < 0.6);
    }

    #[test]
    fn leading_and_trailing_blank_lines_are_trimmed() {
        assert_eq!(merge(&["\n\nalpha\n\n".to_string()]), "alpha");
    }
}
