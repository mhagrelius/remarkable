//! Is the Markdown actually Markdown?
//!
//! These are the checks that need no ground truth. A transcript can be word
//! perfect and still be useless — wrapped in a code fence, or with a `---`
//! between every bullet, or with the same line repeated forty times because the
//! model fell into a loop. Every rule here corresponds to something the prompt
//! asks for, so a failure says either the instruction stopped working or the
//! post-processing did.

use crate::model::text;

/// A rule that was broken, and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fault {
    pub rule: &'static str,
    pub detail: String,
}

impl std::fmt::Display for Fault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.rule, self.detail)
    }
}

/// Every way this transcript is malformed. Empty is a pass.
pub fn faults(transcript: &str) -> Vec<Fault> {
    let mut faults = Vec::new();
    let lines: Vec<&str> = transcript.lines().collect();

    if transcript.trim().is_empty() {
        return vec![Fault {
            rule: "not empty",
            detail: "the transcript is empty".into(),
        }];
    }

    if transcript.trim_start().starts_with("```") {
        faults.push(Fault {
            rule: "no code fence",
            detail: "the whole answer is wrapped in a fence".into(),
        });
    }

    if let Some(preamble) = preamble(transcript) {
        faults.push(Fault {
            rule: "no preamble",
            detail: format!("opens with conversational filler: {preamble:?}"),
        });
    }

    if let Some(at) = rule_between_bullets(&lines) {
        faults.push(Fault {
            rule: "no rule between bullets",
            detail: format!("a `---` sits between two list items at line {}", at + 1),
        });
    }

    if let Some((at, indent)) = odd_indent(&lines) {
        faults.push(Fault {
            rule: "nesting in twos",
            detail: format!("line {} is indented {indent} spaces", at + 1),
        });
    }

    if let Some((at, line, times)) = repetition(&lines) {
        faults.push(Fault {
            rule: "no repetition loop",
            detail: format!("line {} repeats {times} times: {line:?}", at + 1),
        });
    }

    if let Some((first, second, line)) = duplicated_block(&lines) {
        faults.push(Fault {
            rule: "no duplicated seam",
            detail: format!(
                "lines {}.. and {}.. are the same content: {line:?}",
                first + 1,
                second + 1
            ),
        });
    }

    faults
}

/// The model answering rather than transcribing.
fn preamble(transcript: &str) -> Option<String> {
    const OPENERS: [&str; 7] = [
        "here is",
        "here's",
        "sure,",
        "certainly",
        "i've transcribed",
        "the transcription",
        "below is",
    ];
    let first = transcript.lines().find(|line| !line.trim().is_empty())?;
    let lowered = first.trim().to_lowercase();
    OPENERS
        .iter()
        .any(|opener| lowered.starts_with(opener))
        .then(|| first.trim().to_string())
}

fn is_bullet(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("- ")
        || trimmed.starts_with("* ")
        || trimmed
            .split_once(". ")
            .is_some_and(|(number, _)| number.chars().all(|c| c.is_ascii_digit()))
}

fn is_rule(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.len() >= 3 && trimmed.chars().all(|c| c == '-')
}

/// A horizontal rule dropped between two list items — the prompt forbids it,
/// and it is the single most common way this output stops being a list.
fn rule_between_bullets(lines: &[&str]) -> Option<usize> {
    lines.iter().enumerate().position(|(at, line)| {
        if !is_rule(line) {
            return false;
        }
        let before = lines[..at].iter().rev().find(|l| !l.trim().is_empty());
        let after = lines[at + 1..].iter().find(|l| !l.trim().is_empty());
        before.is_some_and(|l| is_bullet(l)) && after.is_some_and(|l| is_bullet(l))
    })
}

/// Nesting the prompt asked for in twos, delivered in threes.
fn odd_indent(lines: &[&str]) -> Option<(usize, usize)> {
    lines.iter().enumerate().find_map(|(at, line)| {
        if !is_bullet(line) {
            return None;
        }
        let indent = line.len() - line.trim_start().len();
        (indent % 2 != 0).then_some((at, indent))
    })
}

/// The same line over and over: a model that ran out of page and kept going.
///
/// Three is the threshold. Two identical bullets are a real thing a person
/// writes; four is never one.
fn repetition(lines: &[&str]) -> Option<(usize, String, usize)> {
    let mut run = 1usize;
    for at in 1..lines.len() {
        let (previous, current) = (lines[at - 1].trim(), lines[at].trim());
        if current.is_empty() || current != previous {
            run = 1;
            continue;
        }
        run += 1;
        if run >= 4 {
            return Some((at + 1 - run, current.to_string(), run));
        }
    }
    None
}

/// A block of lines that appears twice — the overlap between two sections
/// surviving the merge.
///
/// Three consecutive lines, because a page can legitimately repeat one line
/// and even two, but three in the same order is a seam that did not close.
fn duplicated_block(lines: &[&str]) -> Option<(usize, usize, String)> {
    const RUN: usize = 3;

    let meaty: Vec<(usize, &str)> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.trim().len() > 8)
        .map(|(at, line)| (at, *line))
        .collect();

    if meaty.len() < RUN * 2 {
        return None;
    }

    for first in 0..meaty.len() - RUN {
        for second in first + RUN..meaty.len() - RUN + 1 {
            let same = (0..RUN).all(|offset| {
                crate::model::merge::similarity(meaty[first + offset].1, meaty[second + offset].1)
                    > 0.9
            });
            if same {
                return Some((
                    meaty[first].0,
                    meaty[second].0,
                    meaty[first].1.trim().to_string(),
                ));
            }
        }
    }
    None
}

/// Whether the model marked any uncertainty. Not a fault on its own — a clean
/// page of block capitals may honestly have none — but zero across a whole
/// notebook of cursive means the instruction is being ignored.
pub fn uncertainty(transcript: &str) -> usize {
    text::unclear_markers(transcript)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules_broken(transcript: &str) -> Vec<&'static str> {
        faults(transcript).into_iter().map(|f| f.rule).collect()
    }

    #[test]
    fn a_well_formed_transcript_has_no_faults() {
        let good = "## Current Needs\n\n1/27/26\n\n- F & B\n- NFP Applied Epic\n  - Sneha, \
                    [unclear: Anisha] under maximo\n\n---\n\n## AlignTech\n\n- Presales";
        assert_eq!(faults(good), vec![]);
    }

    #[test]
    fn an_empty_transcript_is_a_fault_and_only_that_one() {
        assert_eq!(rules_broken("   \n\n "), vec!["not empty"]);
    }

    #[test]
    fn a_fenced_answer_is_caught() {
        assert!(rules_broken("```markdown\n- a bullet\n```").contains(&"no code fence"));
    }

    #[test]
    fn conversational_filler_is_caught() {
        assert!(rules_broken("Here is the transcription:\n\n- a bullet").contains(&"no preamble"));
        assert!(rules_broken("Sure, I can read that.\n\n- a bullet").contains(&"no preamble"));
    }

    #[test]
    fn a_heading_that_merely_starts_with_a_normal_word_is_not_filler() {
        assert_eq!(
            rules_broken("## Here We Go\n\n- a bullet"),
            Vec::<&str>::new()
        );
    }

    #[test]
    fn a_rule_dropped_between_two_bullets_is_caught() {
        let bad = "- first item\n\n---\n\n- second item";
        assert!(rules_broken(bad).contains(&"no rule between bullets"));
    }

    #[test]
    fn a_rule_dividing_two_sections_is_allowed() {
        let good = "- last item of a section\n\n---\n\n## New Section\n\n- first item";
        assert!(!rules_broken(good).contains(&"no rule between bullets"));
    }

    #[test]
    fn nesting_off_the_two_space_grid_is_caught() {
        let bad = "- Notifications\n   - Teams mobility";
        assert!(rules_broken(bad).contains(&"nesting in twos"));
        let good = "- Notifications\n  - Teams mobility\n    - Needs approval";
        assert!(!rules_broken(good).contains(&"nesting in twos"));
    }

    #[test]
    fn a_repetition_loop_is_caught() {
        let bad = "- a real line\n- stuck\n- stuck\n- stuck\n- stuck";
        assert!(rules_broken(bad).contains(&"no repetition loop"));
    }

    #[test]
    fn a_line_a_person_genuinely_wrote_twice_is_not_a_loop() {
        let fine = "- follow up\n- follow up\n- something else";
        assert!(!rules_broken(fine).contains(&"no repetition loop"));
    }

    #[test]
    fn a_seam_that_did_not_close_is_caught() {
        let bad = "- stay as far away from servers\n- coded apps they like\n\
                   - they prefer that vast simplicity\n- Notifications\n\
                   - stay as far away from servers\n- coded apps they like\n\
                   - they prefer that vast simplicity";
        assert!(rules_broken(bad).contains(&"no duplicated seam"));
    }

    #[test]
    fn a_page_that_lists_the_same_client_twice_is_not_a_seam() {
        let fine = "## Current Needs\n\n- WealthSpire\n- TopBuild\n- Eaton\n\n\
                    ## Notes\n\n- WealthSpire is the priority\n- Eaton can wait";
        assert!(!rules_broken(fine).contains(&"no duplicated seam"));
    }

    #[test]
    fn short_lines_do_not_count_as_a_duplicated_block() {
        // Bare bullets like "- AMH" repeat all over a notebook of client names.
        let fine = "- AMH\n- F & B\n- Eaton\n- more text here to pad\n- AMH\n- F & B\n- Eaton";
        assert!(!rules_broken(fine).contains(&"no duplicated seam"));
    }

    #[test]
    fn uncertainty_markers_are_counted_for_the_report() {
        assert_eq!(uncertainty("[unclear: Sneha] and [unclear: Toby]"), 2);
        assert_eq!(uncertainty("no doubt at all"), 0);
    }
}
