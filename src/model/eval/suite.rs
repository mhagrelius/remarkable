//! The documents the suite grades, and what is written on them.
//!
//! Three exports from the reMarkable web app, at 1,620 pixels wide and 2,160,
//! 9,176 and 17,002 tall. They were chosen because they are not the same
//! problem: the first is one page of neat notes, the second is meeting notes
//! with flow diagrams and struck-through text, and the third is eight pages of
//! fast cursive with boxed headings and client names. A prompt change that
//! helps one and hurts another shows up here.
//!
//! The anchors were read off the images by a person. They are spread through
//! each document rather than clustered at the top, so a section that was skipped
//! or a seam that swallowed a paragraph shows as missing anchors and not as a
//! slightly lower score.

/// One document in the suite.
pub struct Case {
    pub name: &'static str,
    /// Relative to the directory the suite is pointed at.
    pub file: &'static str,
    /// What the page says, in reading order.
    pub anchors: &'static [&'static str],
    /// A page of cursive should produce some. A page of block capitals may
    /// honestly produce none, and `false` says not to complain about it.
    pub expect_uncertainty: bool,
}

pub const CASES: &[Case] = &[
    Case {
        name: "rust-book",
        file: "download.png",
        expect_uncertainty: false,
        anchors: &[
            "The Rust Programming Language Book",
            "Define a Function",
            "fn main",
            "use rustfmt for formatting",
            "println",
            "is a macro",
            "cargo new creates a project",
            "Cargo.toml",
            "holds config",
            "cargo build",
            "defaults to debug",
            "release",
            "cargo run",
            "build and run",
            "std io",
            "prelude",
            "mut",
            "makes a variable mutable",
            "immutable by default",
            "creates comments",
            "accesses items defined on a type",
            "denotes a reference",
            "mut to allow the reference to be mutable",
            "cargo lock",
            "cargo update",
            "cargo doc",
            "open",
            "docs for your dependencies",
        ],
    },
    Case {
        name: "meeting-diagrams",
        file: "download 2.png",
        expect_uncertainty: true,
        anchors: &[
            // The flow sketches at the top.
            "mailbox inbox",
            "mailbox sent",
            "Graph subscription",
            "renew",
            "remove",
            "catchup",
            "out of sync detected",
            "manually triggered",
            "rules based evaluation",
            "LLM based evaluation",
            "sql",
            "outlook addin",
            "how do I file",
            "she is learning the codebase",
            // The WealthSpire section.
            "create burndown list",
            "WealthSpire",
            "iterate quickly",
            "every few days",
            "Next 6 months",
            "Change management",
            "Headless runs IT",
            "Corporate Not Operations",
            "knowledge base",
            "entra groups",
            "power app",
            // Further down.
            "auditing is second",
            "submit",
            "to technology",
            "offboard LoA",
            "leave of absence",
            "Interns",
            "Workday not yet",
            "stay as far away from servers",
            "coded apps they like",
            "Notifications",
            "Teams",
            "Needs approval",
            "phishing",
            "international travel",
        ],
    },
    Case {
        name: "notebook-long",
        file: "download 3.png",
        expect_uncertainty: true,
        anchors: &[
            // The first page.
            "Current Needs",
            "1 27 26",
            "F B",
            "NFP Applied Epic",
            "Black Hills Presales",
            "Greystar SoW",
            "we are up to 18 now",
            "AMH",
            "WealthSpire",
            "TopBuild",
            "Eaton",
            "GMR",
            "large project",
            "Huntington",
            "AlignTech",
            "Presales",
            "Agents in workplace",
            // The last page: reached only if every section was read.
            "workspace",
            "All workspaces have an email",
            "folder",
            "unique on",
            "DMS side",
            "Leave messages in outlook is an option",
            "at end of name after successful link",
            "Filing",
            "in the mailbox view",
            "Only one person has to file a message",
            "Work Panel is path forward for iManage",
            "Addin has a bar on message",
            "recent filing locations",
            "Suggested",
            "Recent",
            "luggage tag in the",
            "BCC",
            "likes moving to linked folder",
        ],
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::eval::expect;
    use crate::model::prompt;
    use crate::model::sections::Section;

    /// Nothing the suite grades on may appear in the prompt.
    ///
    /// This is the check that matters most in the file, because the failure it
    /// prevents is invisible in the results: a phrase written into the
    /// instruction is a phrase the model can emit without having read it off
    /// the page, and recall goes up while accuracy does not.
    ///
    /// It happened. `[Diagram: message → rules based evaluation → LLM based
    /// evaluation → sql]` was the diagram example and handed over three
    /// meeting-diagrams anchors; `[unclear: Sneha] is learning the codebase`
    /// handed over a fourth; and `AlignTech` and `DMS` were added to the
    /// heading and acronym examples *because the eval had just flagged them as
    /// missed*, which turned both green for exactly the wrong reason.
    ///
    /// Graded with `expect::recall`, the same matcher the eval scores with, so
    /// the two cannot disagree about what counts as a match.
    #[test]
    fn the_prompt_gives_away_no_anchor() {
        // The section note is sent too, so it is checked with the base rather
        // than after it. A middle section gets every clause there is.
        let middle = Section {
            top: 0,
            bottom: 100,
            index: 1,
            total: 3,
        };
        let whole = prompt::for_section(&middle, Some("a previous section"));

        for case in CASES {
            let leaked = expect::recall(&whole, case.anchors);
            let given_away: Vec<&str> = leaked
                .hits
                .iter()
                .filter(|hit| hit.found)
                .map(|hit| hit.anchor)
                .collect();
            assert!(
                given_away.is_empty(),
                "the prompt contains {} anchor(s) from {}: {given_away:?}",
                given_away.len(),
                case.name
            );
        }
    }

    /// The guard above, pointed at the wording it was written for.
    ///
    /// A check that has never failed is a check nobody knows the polarity of.
    /// Every line here was in the prompt at some point and must still be
    /// caught.
    #[test]
    fn the_guard_catches_the_leaks_it_was_written_for() {
        let leaked = [
            // Three meeting-diagrams anchors in one example line.
            "[Diagram: message → rules based evaluation → LLM based evaluation → sql]",
            // Both of these were added to the prompt *because* the eval had
            // just reported them missed, which is the worst version of this
            // mistake: it turns a red check green while changing nothing.
            "`[AlignTech]` and `**[AlignTech]**` are wrong; `## AlignTech` is right",
            "an acronym you are inferring → `unique on the [unclear: DMS] side`",
        ];

        for line in leaked {
            let caught = CASES
                .iter()
                .any(|case| expect::recall(line, case.anchors).found() > 0);
            assert!(caught, "the guard would have let this through: {line}");
        }
    }

    /// Why the rule is "no page content", not "no anchor".
    ///
    /// Two lines that were in the prompt and *look* like leaks of
    /// `she is learning the codebase` and `entra groups`, and are not: `sneha
    /// is` does not contain `she is`, and `Entra` alone is not `entra groups`.
    /// The matcher is right about both.
    ///
    /// They still fed the model a name and a product off the page, and most of
    /// a graded phrase. No automated check will ever catch that, because the
    /// space of near-misses is unbounded.
    ///
    /// So the guard is a floor, not a proof. Every example in the prompt is
    /// invented for that reason, and this test records where the floor stops
    /// rather than leaving it to be rediscovered.
    #[test]
    fn a_near_miss_is_not_caught_and_that_is_why_examples_are_invented() {
        for near in [
            "a name you cannot verify → `[unclear: Sneha] is learning the codebase`",
            "a product name — `Entra` read as `entire`",
        ] {
            let caught = CASES
                .iter()
                .any(|case| expect::recall(near, case.anchors).found() > 0);
            assert!(
                !caught,
                "the matcher's behaviour changed; revisit this note"
            );
        }
    }

    #[test]
    fn every_case_names_a_file_and_some_anchors() {
        assert_eq!(CASES.len(), 3);
        for case in CASES {
            assert!(!case.file.is_empty(), "{} has no file", case.name);
            assert!(
                case.anchors.len() >= 15,
                "{} has only {} anchors — too few to notice a skipped section",
                case.name,
                case.anchors.len()
            );
        }
    }

    #[test]
    fn no_anchor_is_so_short_it_matches_by_accident() {
        for case in CASES {
            for anchor in case.anchors {
                assert!(
                    anchor.len() >= 3,
                    "{} in {} is too short to mean anything",
                    anchor,
                    case.name
                );
            }
        }
    }

    #[test]
    fn no_case_lists_the_same_anchor_twice() {
        for case in CASES {
            let mut seen: Vec<&str> = case.anchors.to_vec();
            seen.sort_unstable();
            let before = seen.len();
            seen.dedup();
            assert_eq!(before, seen.len(), "{} repeats an anchor", case.name);
        }
    }

    #[test]
    fn the_long_notebook_is_anchored_at_both_ends() {
        // The point of the last-page anchors: a pipeline that reads the first
        // section and gives up would otherwise score respectably.
        let long = CASES
            .iter()
            .find(|c| c.name == "notebook-long")
            .expect("the case");
        assert!(long.anchors.contains(&"Current Needs"));
        assert!(long.anchors.contains(&"likes moving to linked folder"));
    }
}
