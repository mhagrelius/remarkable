//! Grade the transcription pipeline against a real llama-server.
//!
//!   cargo run --release --example eval
//!   cargo run --release --example eval -- --filter notebook-long
//!   cargo run --release --example eval -- --samples ~/Downloads --write out/
//!
//! This drives the *shipping* code — the same [`Job`], the same prompts, the
//! same [`Client`] the window uses — on a GLib main loop with no display. What
//! it adds is the scoring and a report.
//!
//! Nothing here is a unit test: it needs the GPU awake and takes minutes. The
//! scoring rules it applies are unit-tested in `model::eval`.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Instant;

use gtk::glib;

use remarkable::model::eval::{grade, suite, Score};
use remarkable::model::job::{Job, Lapse, PagePlan};
use remarkable::model::raster::Page;
use remarkable::model::sections::Layout;
use remarkable::ui::client::Client;
use remarkable::ui::runner::{Outcome, Run};
use remarkable::DEFAULT_SERVER;

fn main() -> glib::ExitCode {
    let options = Options::parse();

    let client = Rc::new(Client::new(&options.server));
    let scores: Rc<RefCell<Vec<Score>>> = Rc::new(RefCell::new(Vec::new()));
    let main_loop = glib::MainLoop::new(None, false);

    println!("remarkable eval — {} ", options.server);
    println!("samples: {}\n", options.samples.display());

    // Probe first: a run against a text-only model produces a page of apology
    // and a very confusing report.
    let started = Rc::new(RefCell::new(false));
    client.probe(glib::clone!(
        #[strong]
        client,
        #[strong]
        scores,
        #[strong]
        main_loop,
        #[strong]
        started,
        move |result| {
            match result {
                Ok(info) => {
                    println!(
                        "model:   {} ({})\n",
                        info.model.as_deref().unwrap_or("unnamed"),
                        if info.vision { "vision" } else { "NO VISION" }
                    );
                    if !info.vision {
                        eprintln!("The loaded model has no vision projector. Nothing to grade.");
                        main_loop.quit();
                        return;
                    }
                }
                Err(error) => {
                    eprintln!("llama-server is not answering: {error}");
                    main_loop.quit();
                    return;
                }
            }
            *started.borrow_mut() = true;
            next_case(0, options.clone(), client, scores, main_loop);
        }
    ));

    main_loop.run();

    let scores = scores.borrow();
    if scores.is_empty() {
        return glib::ExitCode::FAILURE;
    }
    report(&scores);

    if scores.iter().all(Score::passed) {
        glib::ExitCode::SUCCESS
    } else {
        glib::ExitCode::FAILURE
    }
}

/// Read one case, score it, and schedule the next. Recursive rather than a
/// loop, because each case finishes in a callback on the main loop.
fn next_case(
    index: usize,
    options: Options,
    client: Rc<Client>,
    scores: Rc<RefCell<Vec<Score>>>,
    main_loop: glib::MainLoop,
) {
    let cases: Vec<&suite::Case> = suite::CASES
        .iter()
        .filter(|case| {
            options
                .filter
                .as_ref()
                .map_or(true, |wanted| case.name.contains(wanted.as_str()))
        })
        .collect();

    let Some(case) = cases.get(index) else {
        main_loop.quit();
        return;
    };

    let path = options.samples.join(case.file);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("  {} — cannot read {}: {error}", case.name, path.display());
            next_case(index + 1, options, client, scores, main_loop);
            return;
        }
    };

    let page = match Page::decode(&bytes) {
        Ok(page) => page,
        Err(error) => {
            eprintln!("  {} — {error}", case.name);
            next_case(index + 1, options, client, scores, main_loop);
            return;
        }
    };

    let layout = Layout::default();
    let plan = PagePlan::from_profile(1, &page.profile(), page.width(), &layout);
    let sections = plan.sections.len();
    println!(
        "{:<18} {}x{} → {sections} section{}",
        case.name,
        page.width(),
        page.height(),
        if sections == 1 { "" } else { "s" }
    );

    let clock = Instant::now();
    let name = case.name;
    let anchors = case.anchors;
    let file = case.file;
    let expect_uncertainty = case.expect_uncertainty;

    Run::start(
        Rc::clone(&client),
        vec![page],
        Job::new(vec![plan]),
        None,
        move |job| {
            let progress = job.progress();
            print!("\r  section {}/{}   ", progress.done, progress.total);
            use std::io::Write as _;
            let _ = std::io::stdout().flush();
        },
        glib::clone!(
            #[strong]
            options,
            #[strong]
            client,
            #[strong]
            scores,
            #[strong]
            main_loop,
            move |job: &Job, outcome: Outcome| {
                let transcript = job.text_so_far();
                let seconds = clock.elapsed().as_secs_f64();
                print!("\r");

                for (section, lapse) in job.lapses() {
                    match lapse {
                        Lapse::Failed(why) => eprintln!("  section {section} failed: {why}"),
                        Lapse::Truncated => eprintln!("  section {section} hit the token ceiling"),
                    }
                }
                if outcome == Outcome::Cancelled {
                    eprintln!("  cancelled");
                }

                if let Some(directory) = &options.write {
                    let _ = std::fs::create_dir_all(directory);
                    let stem = Path::new(file).file_stem().unwrap_or_default();
                    let out = directory.join(format!("{}.md", stem.to_string_lossy()));
                    match std::fs::write(&out, &transcript) {
                        Ok(()) => println!("  written to {}", out.display()),
                        Err(error) => eprintln!("  could not write {}: {error}", out.display()),
                    }
                }

                let score = grade(
                    name,
                    &transcript,
                    anchors,
                    expect_uncertainty,
                    sections,
                    seconds,
                );
                announce(&score);
                scores.borrow_mut().push(score);

                next_case(
                    index + 1,
                    options.clone(),
                    client.clone(),
                    scores.clone(),
                    main_loop.clone(),
                );
            }
        ),
    );
}

fn announce(score: &Score) {
    let verdict = if score.passed() { "PASS" } else { "FAIL" };
    println!(
        "  {verdict}  recall {}/{} ({:.0}%)  {} chars  {} unclear  {:.0}s",
        score.recall.found(),
        score.recall.total(),
        score.recall.fraction() * 100.0,
        score.characters,
        score.uncertainty,
        score.seconds,
    );
    for fault in &score.faults {
        println!("        fault: {fault}");
    }
    for warning in &score.warnings {
        println!("        warn:  {warning}");
    }
    let missing = score.recall.missing();
    if !missing.is_empty() {
        println!("        missed: {}", missing.join(" · "));
    }
    println!();
}

fn report(scores: &[Score]) {
    println!("{:-<80}", "");
    println!(
        "{:<18} {:>7} {:>7} {:>6} {:>8} {:>8} {:>7} {:>6}",
        "case", "recall", "faults", "warn", "unclear", "chars", "secs", "sections"
    );
    for score in scores {
        println!(
            "{:<18} {:>6.0}% {:>7} {:>6} {:>8} {:>8} {:>7.0} {:>6}",
            score.name,
            score.recall.fraction() * 100.0,
            score.faults.len(),
            score.warnings.len(),
            score.uncertainty,
            score.characters,
            score.seconds,
            score.sections,
        );
    }
    println!("{:-<80}", "");

    let passed = scores.iter().filter(|score| score.passed()).count();
    let mean = scores.iter().map(|s| s.recall.fraction()).sum::<f64>() / scores.len() as f64;
    println!(
        "{passed}/{} passed · mean recall {:.0}%",
        scores.len(),
        mean * 100.0
    );
}

#[derive(Clone)]
struct Options {
    server: String,
    samples: PathBuf,
    filter: Option<String>,
    write: Option<PathBuf>,
}

impl Options {
    fn parse() -> Self {
        let mut options = Self {
            server: DEFAULT_SERVER.to_string(),
            samples: glib::home_dir().join("Downloads"),
            filter: None,
            write: None,
        };

        let arguments: Vec<String> = std::env::args().skip(1).collect();
        let mut at = 0;
        while at < arguments.len() {
            let take = |at: usize| arguments.get(at + 1).cloned();
            match arguments[at].as_str() {
                "--server" => {
                    if let Some(value) = take(at) {
                        options.server = value;
                    }
                    at += 1;
                }
                "--samples" => {
                    if let Some(value) = take(at) {
                        options.samples = PathBuf::from(value);
                    }
                    at += 1;
                }
                "--filter" => {
                    options.filter = take(at);
                    at += 1;
                }
                "--write" => {
                    options.write = take(at).map(PathBuf::from);
                    at += 1;
                }
                "--list" => {
                    for case in suite::CASES {
                        println!("{:<18} {}", case.name, case.file);
                    }
                    std::process::exit(0);
                }
                other => eprintln!("ignoring unknown argument {other}"),
            }
            at += 1;
        }
        options
    }
}
