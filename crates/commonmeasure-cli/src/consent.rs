//! `commonmeasure consent`: show, give or withdraw the operator's consent to
//! report to sources whose licence demands it (`commonmeasure_harness::consent`).

use commonmeasure_harness::consent::{self, Answer, Standing};
use commonmeasure_harness::home_dir;
use serde_json::json;
use std::path::Path;

#[derive(clap::Args)]
pub struct Consent {
    #[command(subcommand)]
    action: Option<Action>,
}

#[derive(clap::Subcommand)]
enum Action {
    /// Show the reporting consent recorded in this home and the text it
    /// refers to. The default.
    Show {
        #[arg(long)]
        json: bool,
    },
    /// Agree to report each use of a source whose licence demands reporting.
    /// Such sources are then admitted in every policy scope and reported
    /// through the receiver in relay.json.
    Agree,
    /// Withdraw the consent. Sources that demand reporting are refused from
    /// the next fetch; uses already admitted are still reported.
    Withdraw,
}

pub fn run(args: Consent) -> Result<(), String> {
    let home = home_dir().map_err(|error| error.to_string())?;
    match args.action.unwrap_or(Action::Show { json: false }) {
        Action::Show { json } => {
            show(&home, json);
            Ok(())
        }
        Action::Agree => {
            let given = consent::record(&home, Answer::Agreed, chrono::Utc::now())?;
            println!(
                "reporting consent agreed at {} (consent text {}), recorded in {}",
                stamp(&given.at),
                given.text_version,
                consent::path(&home).display()
            );
            match commonmeasure_harness::relay_config::RelayConfig::load(&home) {
                Ok(Some(_)) => println!(
                    "sources whose licence demands reporting are admitted in every scope and \
                     reported through the receiver in relay.json"
                ),
                Ok(None) => println!(
                    "no receiver is configured in {}, so sources that demand reporting stay \
                     refused until one is (commonmeasure connect)",
                    home.join("relay.json").display()
                ),
                Err(error) => println!(
                    "{error}; sources that demand reporting stay refused until the file loads"
                ),
            }
            Ok(())
        }
        Action::Withdraw => {
            let given = consent::record(&home, Answer::Withdrawn, chrono::Utc::now())?;
            println!(
                "reporting consent withdrawn at {}, recorded in {}",
                stamp(&given.at),
                consent::path(&home).display()
            );
            println!(
                "sources whose licence demands reporting are refused from the next fetch; uses \
                 already admitted are still reported"
            );
            Ok(())
        }
    }
}

fn stamp(at: &chrono::DateTime<chrono::Utc>) -> String {
    at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// One line on the standing, as `status` and `update` print it.
pub fn line(standing: &Standing) -> String {
    match standing {
        Standing::Agreed(given) => format!(
            "reporting consent: agreed at {} (consent text {})",
            stamp(&given.at),
            given.text_version
        ),
        Standing::Withdrawn(given) => format!(
            "reporting consent: withdrawn at {}; sources whose licence demands reporting are \
             refused. Agree with: {}",
            stamp(&given.at),
            consent::AGREE_COMMAND
        ),
        Standing::NotGiven => format!(
            "reporting consent: not given; sources whose licence demands reporting are refused. \
             Agree with: {}",
            consent::AGREE_COMMAND
        ),
        Standing::Unreadable(error) => format!(
            "reporting consent: unreadable ({error}), which is not consent; sources whose \
             licence demands reporting are refused. Record it again with: {}",
            consent::AGREE_COMMAND
        ),
    }
}

fn show(home: &Path, as_json: bool) {
    if as_json {
        let mut report = consent::report(home);
        report["text"] = json!(consent::CONSENT_TEXT);
        println!(
            "{}",
            serde_json::to_string_pretty(&report).expect("a JSON value serialises")
        );
        return;
    }
    println!("{}", line(&Standing::load(home)));
    println!();
    println!("Consent text {}:", consent::CONSENT_TEXT_VERSION);
    println!("{}", consent::CONSENT_TEXT);
}
