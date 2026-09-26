//! `commonmeasure console`: where the operator console is, whether it
//! answers, and the way from the terminal to one of its pages.
//!
//! The console and the command line read the same Edge home. The policy's
//! mode, a scope's denied hosts and the attribution rules are edited in the
//! console's Policy page; `doctor`, `status` and `policy` print what that
//! page shows and edit nothing. This command is the join between the two:
//! it names the console's address as the service installed it, or as
//! `serve` binds by default, asks whether a console answers there, lists
//! the pages by address, and opens one in the browser on request. `doctor`
//! prints the same answer in its Console section, so the operator reading a
//! finding knows where the page that acts on it is.
//!
//! Nothing here starts a console: a server left running by a command that
//! looked like a question would be the wrong surprise. Where none answers,
//! the command names `serve` and, on macOS, the service.

use std::net::SocketAddr;
use std::process::{Command, Stdio};

use commonmeasure_types::Finding;
use serde_json::json;

use crate::report::Palette;
use crate::service::{self, Answer};

#[derive(clap::Args)]
pub struct Console {
    #[command(subcommand)]
    action: Option<Action>,
    /// The address the console listens on, when it is neither the one
    /// `service install console` recorded nor `serve`'s default
    /// 127.0.0.1:4173. Loopback only, as the console is.
    #[arg(long, global = true, value_name = "ADDRESS")]
    listen: Option<String>,
    /// Print the console's address, whether it answers, and the page
    /// addresses as JSON.
    #[arg(long, global = true)]
    json: bool,
}

#[derive(clap::Subcommand)]
enum Action {
    /// Open a page of the console in the browser: the Overview when no page
    /// is named. Refuses when no console answers, naming what starts one.
    Open {
        /// The page: overview, record, agents, policy, sources, compare,
        /// budget or guide.
        #[arg(value_enum)]
        page: Option<Page>,
    },
}

/// The console's pages, as `docs/CONSOLE.md` names them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Page {
    Overview,
    Record,
    Agents,
    Policy,
    Sources,
    Compare,
    Budget,
    Guide,
}

impl Page {
    pub const ALL: [Page; 8] = [
        Page::Overview,
        Page::Record,
        Page::Agents,
        Page::Policy,
        Page::Sources,
        Page::Compare,
        Page::Budget,
        Page::Guide,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Page::Overview => "overview",
            Page::Record => "record",
            Page::Agents => "agents",
            Page::Policy => "policy",
            Page::Sources => "sources",
            Page::Compare => "compare",
            Page::Budget => "budget",
            Page::Guide => "guide",
        }
    }

    /// The path the console serves the page at
    /// (`crates/commonmeasure-console/src/serve.rs`).
    pub fn path(self) -> &'static str {
        match self {
            Page::Overview => "/",
            Page::Record => "/app/record",
            Page::Agents => "/app/agents",
            Page::Policy => "/app/policy",
            Page::Sources => "/app/sources",
            Page::Compare => "/app/compare",
            Page::Budget => "/app/budget",
            Page::Guide => "/guide",
        }
    }

    /// What the page holds, for the listing.
    pub fn holds(self) -> &'static str {
        match self {
            Page::Overview => "crossings, refusals and deliveries; the engagements; the hub card",
            Page::Record => "every crossing by session, with what was refused and why",
            Page::Agents => "the agents and instances that recorded work",
            Page::Policy => {
                "the policy in force per engagement; edits the mode, denied hosts and \
                 attribution rules"
            }
            Page::Sources => "the sources by host, with their terms and standing",
            Page::Compare => "a search run through the configured providers, side by side",
            Page::Budget => "allowances and spend per principal",
            Page::Guide => "the working guide",
        }
    }
}

/// Where the console is and what answered there.
pub struct Located {
    pub address: SocketAddr,
    pub answer: Answer,
}

impl Located {
    pub fn url(&self) -> String {
        format!("http://{}", self.address)
    }

    pub fn page_url(&self, page: Page) -> String {
        format!("{}{}", self.url(), page.path())
    }

    /// Whether a console this binary can send the operator to answers.
    pub fn answers(&self) -> bool {
        matches!(self.answer, Answer::Console { .. })
    }

    /// The finding `doctor` and `console` print for the address: a console
    /// answering with its version and process, one of another version
    /// (which the operator restarts to get the binary they installed),
    /// something else on the port, or nothing.
    pub fn finding(&self, this_version: &str) -> Finding {
        let url = self.url();
        match &self.answer {
            Answer::Console { version, pid } => {
                let pid = pid.map(|pid| format!(", pid {pid}")).unwrap_or_default();
                if version == this_version {
                    Finding::ok(format!(
                        "console: {url} answers, Common Measure {version}{pid}"
                    ))
                } else {
                    Finding::attention(format!(
                        "console: {url} answers with Common Measure {version}{pid}, not this \
                         binary's {this_version}; restart it on this binary (`commonmeasure \
                         service install console` on macOS, or stop it and run `commonmeasure \
                         serve`)"
                    ))
                }
            }
            Answer::Unreported => Finding::unknown(format!(
                "console: something answers on {url} but does not report a version at \
                 /api/version: a console that predates that route, or another program; \
                 `commonmeasure service status` names the process on macOS"
            )),
            Answer::Nothing => Finding::note(format!(
                "console: nothing answers on {url}; `commonmeasure serve` starts it, and \
                 `commonmeasure service install console` keeps it running at login on macOS"
            )),
        }
    }

    pub fn as_json(&self, this_version: &str) -> serde_json::Value {
        let finding = self.finding(this_version);
        let mut document = json!({
            "url": self.url(),
            "answering": self.answers(),
            "standing": finding.standing,
            "text": finding.text,
            "pages": Page::ALL
                .iter()
                .map(|page| (page.name().to_owned(), json!(self.page_url(*page))))
                .collect::<serde_json::Map<String, serde_json::Value>>(),
        });
        if let Answer::Console { version, pid } = &self.answer {
            document["version"] = json!(version);
            document["pid"] = json!(pid);
        }
        document
    }
}

/// Find the console: at `listen` when given, else where the service on
/// this machine installed it, else `serve`'s default. Probes the address
/// once, on loopback, with `service::probe`'s two-second bound.
pub fn locate(listen: Option<&str>) -> Result<Located, String> {
    let listen = match listen {
        Some(listen) => listen.to_owned(),
        None => service::installed_listen().unwrap_or_else(|| service::DEFAULT_LISTEN.to_owned()),
    };
    let address = service::loopback(&listen)?;
    Ok(Located {
        address,
        answer: service::probe(address),
    })
}

pub fn run(args: Console, palette: Palette) -> Result<(), String> {
    let located = locate(args.listen.as_deref())?;
    let version = env!("CARGO_PKG_VERSION");
    match args.action {
        None => {
            if args.json {
                return crate::write_stdout(&format!(
                    "{}\n",
                    serde_json::to_string_pretty(&located.as_json(version))
                        .map_err(|error| error.to_string())?
                ));
            }
            let mut out = palette.finding(&located.finding(version), 0);
            out.push_str(&format!("\n{}\n", palette.heading("Pages")));
            let url_width = Page::ALL
                .iter()
                .map(|page| located.page_url(*page).chars().count())
                .max()
                .unwrap_or_default()
                + 2;
            for page in Page::ALL {
                let url = located.page_url(page);
                out.push_str(&palette.row(
                    None,
                    page.name(),
                    &format!("{url:<url_width$}{}", palette.dim(page.holds())),
                    10,
                ));
            }
            out.push_str("\nopen one with `commonmeasure console open <page>`\n");
            crate::write_stdout(&out)
        }
        Some(Action::Open { page }) => {
            let page = page.unwrap_or(Page::Overview);
            let url = located.page_url(page);
            if !located.answers() {
                return Err(format!(
                    "{}; nothing to open",
                    located.finding(version).text
                ));
            }
            open_in_browser(&url)?;
            crate::write_stdout(&format!("opened {url}\n"))
        }
    }
}

/// Hand a URL to the desktop's opener: `open` on macOS, `xdg-open` on
/// other Unix systems, `start` through `cmd` on Windows. The opener's own
/// output is not ours to print.
fn open_in_browser(url: &str) -> Result<(), String> {
    let (program, args): (&str, Vec<&str>) = if cfg!(target_os = "macos") {
        ("open", vec![url])
    } else if cfg!(windows) {
        ("cmd", vec!["/C", "start", "", url])
    } else {
        ("xdg-open", vec![url])
    };
    let status = Command::new(program)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| format!("cannot run {program} to open {url}: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{program} did not open {url} (exit {status})"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn located(answer: Answer) -> Located {
        Located {
            address: "127.0.0.1:4173".parse().unwrap(),
            answer,
        }
    }

    /// The console's finding names the address and what answered, and a
    /// console of another version is the one state that needs the
    /// operator: the binary they installed is not the one serving.
    #[test]
    fn the_console_finding_says_what_answered_and_flags_another_version() {
        let same = located(Answer::Console {
            version: "0.4.4".into(),
            pid: Some(512),
        })
        .finding("0.4.4");
        assert_eq!(same.standing, commonmeasure_types::Standing::Ok);
        assert_eq!(
            same.text,
            "console: http://127.0.0.1:4173 answers, Common Measure 0.4.4, pid 512"
        );
        let older = located(Answer::Console {
            version: "0.4.3".into(),
            pid: None,
        })
        .finding("0.4.4");
        assert_eq!(older.standing, commonmeasure_types::Standing::Attention);
        assert!(
            older.text.contains("not this binary's 0.4.4"),
            "{}",
            older.text
        );
        let nothing = located(Answer::Nothing).finding("0.4.4");
        assert_eq!(nothing.standing, commonmeasure_types::Standing::Note);
        assert!(nothing.text.contains("`commonmeasure serve` starts it"));
        let other = located(Answer::Unreported).finding("0.4.4");
        assert_eq!(other.standing, commonmeasure_types::Standing::Unknown);
    }

    /// Every page has an address under the console's, and the JSON names
    /// them all with the answer.
    #[test]
    fn the_pages_are_addressed_under_the_console() {
        let console = located(Answer::Console {
            version: "0.4.4".into(),
            pid: Some(1),
        });
        assert_eq!(
            console.page_url(Page::Policy),
            "http://127.0.0.1:4173/app/policy"
        );
        assert_eq!(console.page_url(Page::Overview), "http://127.0.0.1:4173/");
        let json = console.as_json("0.4.4");
        assert_eq!(json["answering"], true);
        assert_eq!(json["version"], "0.4.4");
        assert_eq!(json["pages"]["guide"], "http://127.0.0.1:4173/guide");
        assert_eq!(json["pages"].as_object().unwrap().len(), Page::ALL.len());
    }
}
