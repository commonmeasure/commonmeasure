//! The internal corpus adapter against a real directory on disk — the supply
//! itself, not a stand-in for it. Everything a remote adapter promises holds
//! here too: envelopes derived from the sealed response, hashes recomputable
//! from it, and nothing declared that was not written down.

use std::path::Path;

use commonmeasure_supply::{InternalCorpusAdapter, SupplyAdapter, SupplyError};
use commonmeasure_types::{LicenceState, ProviderCapability};
use serde_json::Value;

fn write(root: &Path, relative: &str, content: &str) {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("mkdir");
    }
    std::fs::write(path, content).expect("write");
}

fn corpus() -> tempfile::TempDir {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    write(
        root,
        "corpus.json",
        r#"{"name": "operator knowledge base",
            "licence": {"state": "declared", "reference": "operator-owned/kb-terms-v1"},
            "dates": {"energy/price-cap.md": "2026-07-30"}}"#,
    );
    write(
        root,
        "energy/price-cap.md",
        "# Price cap briefing\n\nThe Ofgem price cap is reviewed quarterly. The cap limits \
         unit rates, not total bills.\n",
    );
    write(
        root,
        "energy/suppliers.md",
        "# Supplier notes\n\nSupplier standing charges sit under the cap; the cap does not \
         freeze bills.\n",
    );
    write(root, "hr/leave.md", "# Leave policy\n\nBook leave early.\n");
    write(root, "diagram.png.txt", "cap cap cap");
    directory
}

#[test]
fn a_query_ranks_matching_documents_and_derives_envelopes_from_the_sealed_response() {
    let directory = corpus();
    let adapter = InternalCorpusAdapter::new(directory.path());
    assert_eq!(adapter.capabilities(), &[ProviderCapability::Query]);

    let acquisition = adapter
        .query("What does the Ofgem price cap limit?", 2)
        .expect("query succeeds");

    assert_eq!(acquisition.provider, "internal");
    assert_eq!(acquisition.capability, ProviderCapability::Query);
    assert_eq!(
        acquisition.http_status, None,
        "no HTTP happened, so no status may be recorded"
    );
    assert!(acquisition.charge.money.is_none() && acquisition.charge.native.is_none());
    assert!(acquisition.endpoint.starts_with("file://"));

    // Two matches at limit 2; the leave policy does not match the query.
    assert_eq!(acquisition.envelopes.len(), 2);
    let first = &acquisition.envelopes[0];
    assert_eq!(first.title.as_deref(), Some("Price cap briefing"));
    assert_eq!(first.retrieval_rank, 1);
    assert!(first.source_url.starts_with("file://"));
    assert_eq!(
        first.host, "",
        "a file URL has no host, and none is invented"
    );
    assert_eq!(
        first.licence,
        LicenceState::Declared {
            reference: "operator-owned/kb-terms-v1".to_owned()
        },
        "the manifest's declaration travels on every envelope"
    );

    // The operator declared a date for the price-cap briefing and for
    // nothing else. The declaration travels on exactly that envelope, with
    // provenance naming the manifest; the undeclared document stays undated
    // rather than borrowing a filesystem timestamp.
    let declared = first
        .declared_date
        .as_ref()
        .expect("the operator's declared date travels on the envelope");
    assert_eq!(declared.date, "2026-07-30");
    assert!(
        declared.provenance.contains("operator-declared")
            && declared.provenance.contains("corpus.json"),
        "the provenance must name who declared the date: {}",
        declared.provenance
    );
    assert!(
        acquisition.envelopes[1].declared_date.is_none(),
        "a document with no written declaration is undated, not dated by its mtime"
    );

    // The sealed response is what the envelopes were derived from: every
    // content hash must be recomputable from it, as with any provider.
    let sealed: Value = serde_json::from_slice(&acquisition.raw_response).expect("sealed JSON");
    assert_eq!(sealed["documents_scanned"], 4);
    assert_eq!(
        sealed["matches"][0]["declared_date"], "2026-07-30",
        "the envelope's date must be re-derivable from the sealed response"
    );
    assert!(sealed["matches"][1]["declared_date"].is_null());
    for (index, envelope) in acquisition.envelopes.iter().enumerate() {
        let text = sealed["matches"][index]["text"].as_str().expect("text");
        assert_eq!(
            envelope.content_hash.as_deref().unwrap(),
            format!(
                "sha256:{:x}",
                <sha2::Sha256 as sha2::Digest>::digest(text.as_bytes())
            ),
            "the hash must be re-derivable from the sealed response"
        );
    }
}

#[test]
fn the_query_is_deterministic_across_runs() {
    let directory = corpus();
    let adapter = InternalCorpusAdapter::new(directory.path());
    let urls = |acquisition: &commonmeasure_supply::Acquisition| {
        acquisition
            .envelopes
            .iter()
            .map(|envelope| envelope.source_url.clone())
            .collect::<Vec<_>>()
    };
    let first = adapter.query("price cap", 5).expect("query");
    let second = adapter.query("price cap", 5).expect("query");
    assert_eq!(urls(&first), urls(&second));
}

/// Without a written declaration the licence stays unknown. Owning the
/// directory is not a rights statement.
#[test]
fn no_manifest_means_no_licence_claim() {
    let directory = tempfile::tempdir().expect("tempdir");
    write(directory.path(), "note.md", "the cap applies");
    let acquisition = InternalCorpusAdapter::new(directory.path())
        .query("cap", 1)
        .expect("query");
    assert_eq!(acquisition.envelopes[0].licence, LicenceState::Unknown);
}

/// A date declared for a document that is not in the corpus — a typo, a file
/// since deleted — is a declaration lost, and losing it silently would leave
/// that document undated with nobody told.
#[test]
fn a_date_declared_for_a_missing_document_is_an_error_not_a_silent_absence() {
    let directory = tempfile::tempdir().expect("tempdir");
    write(
        directory.path(),
        "corpus.json",
        r#"{"dates": {"energy/price-cap.md": "2026-07-30"}}"#,
    );
    write(directory.path(), "note.md", "the cap applies");
    let error = InternalCorpusAdapter::new(directory.path())
        .query("cap", 1)
        .expect_err("a dangling date declaration must refuse the query");
    match error {
        SupplyError::Malformed { detail } => assert!(
            detail.contains("energy/price-cap.md"),
            "the error names the path the declaration cannot attach to: {detail}"
        ),
        other => panic!("expected a malformed manifest error, got {other:?}"),
    }
}

/// The same loss one step over: a date declared for a file that exists but
/// that the scanner will never scan — the manifest itself, an image —
/// validates against mere existence and then silently never attaches. The
/// check must require a scannable document, not just a file.
#[test]
fn a_date_declared_for_an_unscannable_file_is_an_error_not_a_silent_absence() {
    let directory = tempfile::tempdir().expect("tempdir");
    write(
        directory.path(),
        "corpus.json",
        r#"{"dates": {"diagram.png": "2026-07-30"}}"#,
    );
    write(directory.path(), "diagram.png", "not a document");
    write(directory.path(), "note.md", "the cap applies");
    let error = InternalCorpusAdapter::new(directory.path())
        .query("cap", 1)
        .expect_err("a date the scan can never attach must refuse the query");
    match error {
        SupplyError::Malformed { detail } => assert!(
            detail.contains("diagram.png"),
            "the error names the file the declaration cannot attach to: {detail}"
        ),
        other => panic!("expected a malformed manifest error, got {other:?}"),
    }
}

/// The operator's governance metadata travels from the manifest through the
/// sealed response onto exactly the envelope it was declared for; a document
/// with no declaration carries none — absent, never defaulted.
#[test]
fn declared_governance_metadata_reaches_the_envelope_and_absence_stays_absent() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    write(
        root,
        "corpus.json",
        r#"{"documents": {"flux.md": {
              "edition": "orchestrator", "version_range": "4.x",
              "integration_path": "flux-connector",
              "support_status": "deprecated", "entitlement": "standard"}}}"#,
    );
    write(root, "flux.md", "# Flux\n\nthe cap on flux throughput\n");
    write(root, "plain.md", "# Plain\n\nthe cap on plain throughput\n");

    let acquisition = InternalCorpusAdapter::new(root)
        .query("cap throughput", 5)
        .expect("query");
    assert_eq!(acquisition.envelopes.len(), 2);
    let by_path = |name: &str| {
        acquisition
            .envelopes
            .iter()
            .find(|envelope| envelope.native_metadata["path"] == name)
            .expect("envelope present")
    };

    let governance = &by_path("flux.md").native_metadata["governance"];
    assert_eq!(governance["edition"], "orchestrator");
    assert_eq!(governance["version_range"], "4.x");
    assert_eq!(governance["integration_path"], "flux-connector");
    assert_eq!(governance["support_status"], "deprecated");
    assert_eq!(governance["entitlement"], "standard");
    assert!(
        by_path("plain.md").native_metadata["governance"].is_null(),
        "an undeclared document carries no governance metadata"
    );

    // Re-derivable from the sealed response, like every envelope field.
    let sealed: Value = serde_json::from_slice(&acquisition.raw_response).expect("sealed JSON");
    let sealed_governance = sealed["matches"]
        .as_array()
        .unwrap()
        .iter()
        .find(|matched| matched["path"] == "flux.md")
        .expect("match present")["governance"]
        .clone();
    assert_eq!(&sealed_governance, governance);
}

/// Governance metadata declared for a document that is not in the corpus is
/// a declaration lost, exactly as a dangling date is: an error, never a
/// silent absence.
#[test]
fn governance_metadata_declared_for_a_missing_document_is_an_error() {
    let directory = tempfile::tempdir().expect("tempdir");
    write(
        directory.path(),
        "corpus.json",
        r#"{"documents": {"gone.md": {
              "edition": "orchestrator", "version_range": "4.x",
              "integration_path": "flux-connector",
              "support_status": "deprecated", "entitlement": "standard"}}}"#,
    );
    write(directory.path(), "note.md", "the cap applies");
    let error = InternalCorpusAdapter::new(directory.path())
        .query("cap", 1)
        .expect_err("a dangling governance declaration must refuse the query");
    match error {
        SupplyError::Malformed { detail } => assert!(
            detail.contains("gone.md") && detail.contains("governance"),
            "the error names the path and the kind of declaration lost: {detail}"
        ),
        other => panic!("expected a malformed manifest error, got {other:?}"),
    }
}

/// A per-document licence declaration travels from the manifest through the
/// sealed response onto exactly the envelope it was declared for; a document
/// without one falls back to the corpus-wide state. One corpus can then
/// honestly mix rights — some documents licensed, some nobody's to license —
/// without the corpus-wide state misdeclaring either side.
#[test]
fn a_per_document_licence_outranks_the_corpus_wide_state_and_absence_falls_back() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    write(
        root,
        "corpus.json",
        r#"{"licence": {"state": "declared", "reference": "retailer/first-party-v1"},
            "licences": {"scraped-review.md": {"state": "unknown"}}}"#,
    );
    write(root, "own-page.md", "# Own page\n\nthe cap on kettles\n");
    write(
        root,
        "scraped-review.md",
        "# Review\n\nthe cap on kettles reviewed\n",
    );

    let acquisition = InternalCorpusAdapter::new(root)
        .query("cap kettles", 5)
        .expect("query");
    assert_eq!(acquisition.envelopes.len(), 2);
    let by_path = |name: &str| {
        acquisition
            .envelopes
            .iter()
            .find(|envelope| envelope.native_metadata["path"] == name)
            .expect("envelope present")
    };
    assert_eq!(
        by_path("own-page.md").licence,
        LicenceState::Declared {
            reference: "retailer/first-party-v1".to_owned()
        },
        "a document with no per-document declaration carries the corpus-wide state"
    );
    assert_eq!(
        by_path("scraped-review.md").licence,
        LicenceState::Unknown,
        "the per-document declaration outranks the corpus-wide state"
    );

    // Re-derivable from the sealed response, like every envelope field: the
    // declared document's match carries its own licence, the other's is null
    // and the corpus-wide state at the response's top level speaks for it.
    let sealed: Value = serde_json::from_slice(&acquisition.raw_response).expect("sealed JSON");
    let match_of = |name: &str| {
        sealed["matches"]
            .as_array()
            .unwrap()
            .iter()
            .find(|matched| matched["path"] == name)
            .expect("match present")
            .clone()
    };
    assert_eq!(match_of("scraped-review.md")["licence"]["state"], "unknown");
    assert!(match_of("own-page.md")["licence"].is_null());
    assert_eq!(sealed["licence"]["reference"], "retailer/first-party-v1");
}

/// Without a corpus-wide default, a per-document declaration licenses exactly
/// the documents it names and nothing else: the rest stay unknown, because
/// owning the directory is not a rights statement.
#[test]
fn per_document_licences_without_a_default_leave_the_rest_unknown() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    write(
        root,
        "corpus.json",
        r#"{"licences": {"catalogue.md": {"state": "declared", "reference": "retailer/catalogue-v1"}}}"#,
    );
    write(root, "catalogue.md", "# Catalogue\n\nthe cap listed\n");
    write(root, "clipping.md", "# Clipping\n\nthe cap clipped\n");

    let acquisition = InternalCorpusAdapter::new(root)
        .query("cap", 5)
        .expect("query");
    let by_path = |name: &str| {
        acquisition
            .envelopes
            .iter()
            .find(|envelope| envelope.native_metadata["path"] == name)
            .expect("envelope present")
    };
    assert_eq!(
        by_path("catalogue.md").licence,
        LicenceState::Declared {
            reference: "retailer/catalogue-v1".to_owned()
        }
    );
    assert_eq!(by_path("clipping.md").licence, LicenceState::Unknown);
}

/// A licence declared for a document that is not in the corpus is a
/// declaration lost, exactly as a dangling date is: an error, never a silent
/// absence — silently dropping it would leave the document's rights
/// misdeclared with nobody told.
#[test]
fn a_licence_declared_for_a_missing_document_is_an_error() {
    let directory = tempfile::tempdir().expect("tempdir");
    write(
        directory.path(),
        "corpus.json",
        r#"{"licences": {"gone.md": {"state": "declared", "reference": "retailer/v1"}}}"#,
    );
    write(directory.path(), "note.md", "the cap applies");
    let error = InternalCorpusAdapter::new(directory.path())
        .query("cap", 1)
        .expect_err("a dangling licence declaration must refuse the query");
    match error {
        SupplyError::Malformed { detail } => assert!(
            detail.contains("gone.md") && detail.contains("licence"),
            "the error names the path and the kind of declaration lost: {detail}"
        ),
        other => panic!("expected a malformed manifest error, got {other:?}"),
    }
}

/// A misspelled field inside a governance declaration is a declaration lost,
/// not an absent one: unknown fields are load errors all the way down.
#[test]
fn an_unknown_field_in_a_governance_declaration_is_an_error() {
    let directory = tempfile::tempdir().expect("tempdir");
    write(
        directory.path(),
        "corpus.json",
        r#"{"documents": {"note.md": {
              "edition": "orchestrator", "version_range": "4.x",
              "integration_path": "flux-connector",
              "support_status": "deprecated", "entitlement": "standard",
              "entitelment": "premier"}}}"#,
    );
    write(directory.path(), "note.md", "the cap applies");
    let result = InternalCorpusAdapter::new(directory.path()).query("cap", 1);
    assert!(matches!(result, Err(SupplyError::Malformed { .. })));
}

/// The committed specialist bundle (`demo/specialist/corpus/`) loads through
/// the real adapter: its manifest validates, its licence is declared, and
/// its governance declarations attach where they were written. This is the
/// corpus the governed specialist runs are made from, so a dangling declaration
/// must fail here, not in a run.
#[test]
fn the_committed_specialist_bundle_loads_and_carries_its_declarations() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../demo/specialist/corpus");
    let acquisition = InternalCorpusAdapter::new(&root)
        .query("flux connector orchestrator data sources", 10)
        .expect("the committed bundle must load");
    assert_eq!(
        acquisition.envelopes[0].licence,
        LicenceState::Declared {
            reference: "fictive-systems/doc-licence-v1".to_owned()
        }
    );
    let governance = |name: &str| {
        acquisition
            .envelopes
            .iter()
            .find(|envelope| envelope.native_metadata["path"] == name)
            .unwrap_or_else(|| panic!("{name} should match this query"))
            .native_metadata["governance"]
            .clone()
    };
    let deprecated = governance("flux-connector-4x-migration.md");
    assert_eq!(deprecated["support_status"], "deprecated");
    assert_eq!(deprecated["integration_path"], "flux-connector");
    assert!(
        governance("orchestrator-overview.md").is_null(),
        "the overview deliberately declares no governance metadata"
    );
    for envelope in &acquisition.envelopes {
        assert!(
            envelope.declared_date.is_some(),
            "every specialist document declares an effective date: {}",
            envelope.native_metadata["path"]
        );
    }
}

/// The committed commerce bundle (`demo/commerce/corpus/`) loads through the
/// real adapter at its mixed root and at each class root, and the mixed
/// manifest carries the per-document licence state the commerce comparison's governed condition
/// turns on: every product, brand and syndicated editorial document declared,
/// the GadgetGrove capture alone unknown, because nobody licensed it.
#[test]
fn the_committed_commerce_bundle_loads_and_mixes_rights_honestly() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../demo/commerce/corpus");
    let acquisition = InternalCorpusAdapter::new(&root)
        .query(
            "aurelio presto espresso machine northglade grinder kitchens",
            10,
        )
        .expect("the committed bundle must load");
    let by_path = |name: &str| {
        acquisition
            .envelopes
            .iter()
            .find(|envelope| envelope.native_metadata["path"] == name)
            .unwrap_or_else(|| panic!("{name} should match this query"))
    };
    assert_eq!(
        by_path("product-data/presto-300-listing.md").licence,
        LicenceState::Declared {
            reference: "fictive-retail/product-data-licence-v1".to_owned()
        }
    );
    assert_eq!(
        by_path("third-party/crema-courier-roundup.md").licence,
        LicenceState::Declared {
            reference: "crema-courier/syndication-licence-v1".to_owned()
        }
    );
    assert_eq!(
        by_path("third-party/gadgetgrove-presto-300-review.md").licence,
        LicenceState::Unknown,
        "the unlicensed capture must not inherit a licence from the documents beside it"
    );
    for envelope in &acquisition.envelopes {
        assert!(
            envelope.declared_date.is_some(),
            "every commerce document declares a date: {}",
            envelope.native_metadata["path"]
        );
    }
}

/// The class roots are runnable corpora of their own (the class-alone
/// conditions), so each class declares its documents twice: in its own
/// manifest and, class-prefixed, in the mixed root's. Two manifests that
/// drift would date or license one document two ways depending on which
/// condition read it — this pin makes the drift a test failure instead.
#[test]
fn the_commerce_class_manifests_agree_with_the_mixed_root_manifest() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../demo/commerce/corpus");
    let manifest = |path: &Path| -> Value {
        serde_json::from_slice(&std::fs::read(path.join("corpus.json")).expect("read manifest"))
            .expect("parse manifest")
    };
    let mixed = manifest(&root);
    let mut mixed_dates = 0usize;
    let mut mixed_licences = 0usize;
    for class in ["product-data", "brand-content", "third-party"] {
        let class_manifest = manifest(&root.join(class));
        // The class manifest may state its licence corpus-wide where every
        // document in the class shares it; the mixed root must state the
        // same licence per document.
        let class_default = class_manifest.get("licence").cloned();
        let dates = class_manifest["dates"].as_object().expect("dates");
        for (name, date) in dates {
            mixed_dates += 1;
            assert_eq!(
                &mixed["dates"][format!("{class}/{name}")],
                date,
                "{class}/{name} is dated differently by the class and mixed manifests"
            );
            let class_licence = class_manifest["licences"]
                .get(name)
                .cloned()
                .or_else(|| class_default.clone());
            let mixed_licence = mixed["licences"].get(format!("{class}/{name}")).cloned();
            assert_eq!(
                mixed_licence, class_licence,
                "{class}/{name} is licensed differently by the class and mixed manifests"
            );
            if mixed_licence.is_some() {
                mixed_licences += 1;
            }
        }
    }
    assert_eq!(
        mixed["dates"].as_object().expect("dates").len(),
        mixed_dates,
        "the mixed manifest dates a document no class manifest dates"
    );
    assert_eq!(
        mixed["licences"].as_object().expect("licences").len(),
        mixed_licences,
        "the mixed manifest licenses a document no class manifest licenses"
    );
}

/// A manifest that cannot be read is an error, never a quiet "no licence":
/// the operator wrote a rights statement and losing it would misdeclare
/// every envelope.
#[test]
fn a_malformed_manifest_is_an_error_not_a_silent_unknown() {
    let directory = tempfile::tempdir().expect("tempdir");
    write(directory.path(), "corpus.json", "{ not json");
    write(directory.path(), "note.md", "text");
    let result = InternalCorpusAdapter::new(directory.path()).query("text", 1);
    assert!(matches!(result, Err(SupplyError::Malformed { .. })));
}

/// An oversized document is reported as skipped in the sealed response, never
/// silently missing from the corpus it belongs to.
#[test]
fn an_oversized_document_is_skipped_and_the_response_says_so() {
    let directory = tempfile::tempdir().expect("tempdir");
    write(directory.path(), "small.md", "cap notes");
    write(directory.path(), "huge.md", &"cap ".repeat(300_000));
    let acquisition = InternalCorpusAdapter::new(directory.path())
        .query("cap", 5)
        .expect("query");
    assert_eq!(acquisition.envelopes.len(), 1);
    let sealed: Value = serde_json::from_slice(&acquisition.raw_response).unwrap();
    assert_eq!(sealed["skipped"][0]["path"], "huge.md");
    assert!(
        sealed["skipped"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("bound"),
        "the skip names its reason"
    );
}

/// An unreadable document is a fact about the scan, not a smaller corpus: it
/// lands in `skipped` with the OS error, exactly as a non-UTF-8 document does.
/// Before this held, an I/O failure was published as a coverage fact — a
/// sealed response claiming to name every document scanned while missing one
/// nobody scanned.
#[test]
#[cfg(unix)]
fn an_unreadable_document_is_skipped_and_the_response_says_so() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().expect("tempdir");
    write(directory.path(), "open.md", "cap notes");
    write(directory.path(), "sealed-off.md", "cap secrets");
    std::fs::set_permissions(
        directory.path().join("sealed-off.md"),
        std::fs::Permissions::from_mode(0o000),
    )
    .expect("chmod");

    let acquisition = InternalCorpusAdapter::new(directory.path())
        .query("cap", 5)
        .expect("query");
    assert_eq!(acquisition.envelopes.len(), 1);
    let sealed: Value = serde_json::from_slice(&acquisition.raw_response).unwrap();
    assert_eq!(sealed["skipped"][0]["path"], "sealed-off.md");
    assert!(
        sealed["skipped"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("cannot read"),
        "the skip names its reason"
    );
    // Discovered and scanned are different facts: the unreadable document was
    // found but never read, and the counts must not claim otherwise.
    assert_eq!(sealed["documents_discovered"], 2);
    assert_eq!(sealed["documents_scanned"], 1);
    assert_eq!(sealed["skipped"].as_array().unwrap().len(), 1);
}

/// A symlink inside the corpus pointing outside it is a pointer, not a
/// declaration: neither a file nor a directory escape is read or admitted,
/// and the sealed response records each exclusion. Without the check an
/// escaping symlink's target is sealed and admitted as corpus content.
#[test]
#[cfg(unix)]
fn a_symlink_escaping_the_corpus_root_is_excluded_and_the_response_says_so() {
    let outside = tempfile::tempdir().expect("tempdir");
    write(outside.path(), "secret.md", "cap secrets kept elsewhere");
    let directory = tempfile::tempdir().expect("tempdir");
    write(directory.path(), "open.md", "cap notes");
    std::os::unix::fs::symlink(
        outside.path().join("secret.md"),
        directory.path().join("stray.md"),
    )
    .expect("file symlink");
    std::os::unix::fs::symlink(outside.path(), directory.path().join("stray-dir"))
        .expect("directory symlink");

    let acquisition = InternalCorpusAdapter::new(directory.path())
        .query("cap", 5)
        .expect("query");
    assert_eq!(acquisition.envelopes.len(), 1);
    assert!(
        acquisition.envelopes[0].source_url.ends_with("open.md"),
        "only the declared document may be admitted"
    );
    let sealed: Value = serde_json::from_slice(&acquisition.raw_response).unwrap();
    assert!(
        !String::from_utf8_lossy(&acquisition.raw_response).contains("kept elsewhere"),
        "no byte of the escape target may be sealed"
    );
    let skipped_paths: Vec<&str> = sealed["skipped"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|entry| entry["path"].as_str())
        .collect();
    assert!(
        skipped_paths.contains(&"stray.md"),
        "got: {skipped_paths:?}"
    );
    assert!(
        skipped_paths.contains(&"stray-dir"),
        "got: {skipped_paths:?}"
    );
    assert!(
        sealed["skipped"]
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| entry["reason"]
                .as_str()
                .unwrap()
                .contains("outside the corpus root")),
        "each exclusion names its reason"
    );
    assert_eq!(sealed["documents_discovered"], 1);
    assert_eq!(sealed["documents_scanned"], 1);
}

/// A directory symlink cycle terminates instead of recursing to exhaustion.
#[test]
#[cfg(unix)]
fn a_directory_symlink_cycle_terminates() {
    let directory = tempfile::tempdir().expect("tempdir");
    write(directory.path(), "docs/open.md", "cap notes");
    std::os::unix::fs::symlink(directory.path(), directory.path().join("docs/loop"))
        .expect("cycle symlink");

    let acquisition = InternalCorpusAdapter::new(directory.path())
        .query("cap", 5)
        .expect("query");
    assert_eq!(acquisition.envelopes.len(), 1);
    let sealed: Value = serde_json::from_slice(&acquisition.raw_response).unwrap();
    assert_eq!(sealed["documents_discovered"], 1);
    assert_eq!(sealed["documents_scanned"], 1);
}

/// One file reachable under two names is one document. Before this held, an
/// alias beside its target was discovered, hashed and admitted twice with
/// identical content, the coverage record measured the job against a corpus
/// larger than the one on disk, and which of the two names reached the model
/// depended on directory order.
#[test]
#[cfg(unix)]
fn a_document_reachable_under_two_names_is_one_document() {
    let directory = tempfile::tempdir().expect("tempdir");
    write(
        directory.path(),
        "price-cap.md",
        "# Price cap\n\nthe cap applies\n",
    );
    std::os::unix::fs::symlink(
        directory.path().join("price-cap.md"),
        directory.path().join("alias.md"),
    )
    .expect("file symlink");

    let acquisition = InternalCorpusAdapter::new(directory.path())
        .query("cap", 5)
        .expect("query");

    assert_eq!(acquisition.envelopes.len(), 1, "one document, one envelope");
    assert!(
        acquisition.envelopes[0]
            .source_url
            .ends_with("price-cap.md"),
        "provenance names the document, not the name it was reached by: {}",
        acquisition.envelopes[0].source_url
    );
    let sealed: Value = serde_json::from_slice(&acquisition.raw_response).unwrap();
    assert_eq!(sealed["documents_discovered"], 1);
    assert_eq!(sealed["documents_scanned"], 1);
}

/// A name resolved during the walk is the name read at scan time. A symlink
/// flipped in between must not be able to substitute a file from outside the
/// root: the resolution that passed containment and the read that produced the
/// bytes have to be the same one.
#[test]
#[cfg(unix)]
fn a_symlink_flipped_during_the_walk_cannot_supply_content_from_outside_the_root() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    let outside = tempfile::tempdir().expect("tempdir");
    write(outside.path(), "secret.md", "cap secrets kept elsewhere");
    let directory = tempfile::tempdir().expect("tempdir");
    write(
        directory.path(),
        "inside.md",
        "cap notes held in the corpus",
    );

    // Documents sorting ahead of the link, so the swap has the whole of the
    // rest of the scan to land in — the window a second resolution opens.
    for index in 0..40 {
        write(
            directory.path(),
            &format!("filler-{index:03}.md"),
            "corpus filler with no query terms in it at all\n",
        );
    }

    let link = directory.path().join("flip.md");
    // Replaced by rename so the link always names one target or the other, and
    // the walk never sees the moment of the swap.
    let point = |target: &Path| {
        let staging = link.with_extension("staging");
        let _ = std::fs::remove_file(&staging);
        std::os::unix::fs::symlink(target, &staging).expect("symlink");
        std::fs::rename(&staging, &link).expect("rename");
    };
    point(&directory.path().join("inside.md"));

    let stop = Arc::new(AtomicBool::new(false));
    let flipper = {
        let stop = Arc::clone(&stop);
        let inside = directory.path().join("inside.md");
        let elsewhere = outside.path().join("secret.md");
        let link = link.clone();
        std::thread::spawn(move || {
            let mut outward = true;
            while !stop.load(Ordering::Relaxed) {
                let target = if outward { &elsewhere } else { &inside };
                let staging = link.with_extension("staging");
                let _ = std::fs::remove_file(&staging);
                if std::os::unix::fs::symlink(target, &staging).is_ok() {
                    let _ = std::fs::rename(&staging, &link);
                }
                outward = !outward;
                std::thread::sleep(std::time::Duration::from_micros(50));
            }
        })
    };

    let adapter = InternalCorpusAdapter::new(directory.path());
    for run in 0..60 {
        let acquisition = adapter.query("cap", 5).expect("query");
        let sealed = String::from_utf8_lossy(&acquisition.raw_response).into_owned();
        assert!(
            !sealed.contains("kept elsewhere"),
            "run {run} sealed content from outside the corpus root"
        );
        for envelope in &acquisition.envelopes {
            assert!(
                !envelope
                    .text
                    .as_deref()
                    .unwrap_or_default()
                    .contains("kept elsewhere"),
                "run {run} admitted content from outside the corpus root"
            );
        }
    }
    stop.store(true, Ordering::Relaxed);
    flipper.join().expect("flipper thread");
}

/// A corpus entry that is not a regular file is skipped on the strength of the
/// stat already taken. A FIFO named `pipe.md` measures zero bytes and passes
/// every size test; reading it blocks until someone writes, which would hang
/// the query for ever and leave no record of the run at all.
#[test]
#[cfg(unix)]
fn a_named_pipe_is_skipped_and_the_query_returns() {
    let directory = tempfile::tempdir().expect("tempdir");
    write(directory.path(), "open.md", "cap notes");
    let made = std::process::Command::new("mkfifo")
        .arg(directory.path().join("pipe.md"))
        .status()
        .expect("mkfifo runs");
    assert!(made.success(), "mkfifo should create the pipe");

    let root = directory.path().to_path_buf();
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(InternalCorpusAdapter::new(&root).query("cap", 5));
    });
    let acquisition = receiver
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("the query must return rather than block on a pipe")
        .expect("query");

    assert_eq!(acquisition.envelopes.len(), 1);
    let sealed: Value = serde_json::from_slice(&acquisition.raw_response).unwrap();
    assert_eq!(sealed["skipped"][0]["path"], "pipe.md");
    assert!(
        sealed["skipped"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("regular file"),
        "the skip names its reason"
    );
    assert_eq!(sealed["documents_discovered"], 1);
    assert_eq!(sealed["documents_scanned"], 1);
}

/// The manifest is under the same containment as the documents it speaks for.
/// A `corpus.json` resolving outside the root would declare rights over every
/// envelope from a file the operator never put in the corpus.
#[test]
#[cfg(unix)]
fn a_manifest_resolving_outside_the_corpus_root_is_refused() {
    let outside = tempfile::tempdir().expect("tempdir");
    write(
        outside.path(),
        "planted.json",
        r#"{"licence": {"state": "declared", "reference": "PLANTED-LICENCE"}}"#,
    );
    let directory = tempfile::tempdir().expect("tempdir");
    write(directory.path(), "note.md", "the cap applies");
    std::os::unix::fs::symlink(
        outside.path().join("planted.json"),
        directory.path().join("corpus.json"),
    )
    .expect("manifest symlink");

    let result = InternalCorpusAdapter::new(directory.path()).query("cap", 1);
    match result {
        Err(SupplyError::Transport { detail }) => assert!(
            detail.contains("outside the corpus root"),
            "the refusal names its reason: {detail}"
        ),
        other => panic!("an escaping manifest must be refused, got {other:?}"),
    }
}

/// A manifest that is present but unreadable is an error, not an absence: a
/// dangling `corpus.json` read as absent would answer "no licence declared", which is the
/// silent unknown a malformed manifest is already forbidden from producing.
#[test]
#[cfg(unix)]
fn a_dangling_manifest_symlink_is_an_error_not_a_silent_unknown() {
    let directory = tempfile::tempdir().expect("tempdir");
    write(directory.path(), "note.md", "the cap applies");
    std::os::unix::fs::symlink(
        directory.path().join("nowhere.json"),
        directory.path().join("corpus.json"),
    )
    .expect("manifest symlink");

    let result = InternalCorpusAdapter::new(directory.path()).query("cap", 1);
    assert!(
        matches!(result, Err(SupplyError::Transport { .. })),
        "got {result:?}"
    );
}

/// A missing corpus root is an explicit unavailable, naming the path.
#[test]
fn a_missing_corpus_root_is_an_explicit_error() {
    let result =
        InternalCorpusAdapter::new(Path::new("/nonexistent/corpus/root")).query("anything", 1);
    assert!(matches!(result, Err(SupplyError::Transport { .. })));
}

/// The adapter declares `query` alone, and its `search` answers as the
/// contract requires: a named missing capability, not an improvised search.
#[test]
fn search_is_not_pretended() {
    let directory = corpus();
    let result = InternalCorpusAdapter::new(directory.path()).search("cap", 1, &[]);
    assert!(matches!(
        result,
        Err(SupplyError::CapabilityUnavailable { .. })
    ));
}
