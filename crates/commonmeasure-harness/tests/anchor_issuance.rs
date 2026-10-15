//! Replay of a recorded issuance: the stand-in Anchor's directory, three
//! passes and its ration refusal (`tests/recorded/anchor-issuance/`), served
//! from loopback to the edge's own client, transport and wallet.
//!
//! The recording was made with a seeded generator, which this test seeds
//! the same way, so every request the client sends is the recorded one byte
//! for byte apart from the loopback port in `Host`. That is also what holds
//! the unlinkability claim: no request to the Anchor carries the edge's
//! signature headers or a user agent.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use blind_rsa_signatures::reexports::rand::SeedableRng as _;
use blind_rsa_signatures::reexports::rand::rngs::StdRng;
use commonmeasure_harness::SessionLog;
use commonmeasure_harness::passes::{
    PublicKey, PublicToken, Stopped, Wallet, challenge_for, source_challenges,
};
use commonmeasure_http::{Request, Response, Server, read_request, read_response, write_request};
use privacypass::Deserialize as _;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn recorded() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/recorded/anchor-issuance")
}

/// The recorded exchanges in order, each request and answer as its bytes.
fn exchanges() -> Vec<(Vec<u8>, Vec<u8>)> {
    (1..)
        .map(|n| {
            let dir = recorded();
            (
                std::fs::read(dir.join(format!("{n:02}.request"))),
                std::fs::read(dir.join(format!("{n:02}.response"))),
            )
        })
        .take_while(|(request, _)| request.is_ok())
        .map(|(request, response)| (request.unwrap(), response.expect("an answer per request")))
        .collect()
}

/// A request as bytes with its `Host` line, the one place a loopback port
/// appears, written as `Host: anchor`.
fn without_port(bytes: &[u8]) -> Vec<u8> {
    let end = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("a request head")
        + 4;
    let head = std::str::from_utf8(&bytes[..end]).expect("a text head");
    let head: String = head
        .split_inclusive("\r\n")
        .map(|line| {
            if line.to_ascii_lowercase().starts_with("host:") {
                "Host: anchor\r\n"
            } else {
                line
            }
        })
        .collect();
    [head.as_bytes(), &bytes[end..]].concat()
}

/// RFC 9578 §5.3: the authenticator signs token_type, nonce,
/// challenge_digest and token_key_id, concatenated.
fn verifies(token: &PublicToken, key: &PublicKey) -> bool {
    let mut input = Vec::new();
    input.extend_from_slice(&(token.token_type() as u16).to_be_bytes());
    input.extend_from_slice(&token.nonce());
    input.extend_from_slice(token.challenge_digest());
    input.extend_from_slice(token.token_key_id());
    key.verify(
        &blind_rsa_signatures::Signature(token.authenticator().to_vec()),
        None,
        &input,
    )
    .is_ok()
}

#[test]
fn a_recorded_issuance_fills_the_wallet_unsigned() {
    let recording: Value = serde_json::from_slice(
        &std::fs::read(recorded().join("recording.json")).expect("the recording"),
    )
    .expect("JSON");
    let credential = recording["credential"].as_str().unwrap();
    let host = recording["host"].as_str().unwrap();
    let count = recording["count"].as_u64().unwrap() as usize;
    let exchanges = exchanges();
    assert_eq!(
        exchanges.len(),
        1 + count,
        "a directory read and each request"
    );

    // The Anchor, as recorded: each request answered in turn with the
    // recorded answer, and every request kept as it arrived.
    let received: Arc<Mutex<Vec<Request>>> = Arc::default();
    let answers: Vec<Response> = exchanges
        .iter()
        .map(|(_, answer)| read_response(&mut answer.as_slice()).expect("a recorded answer"))
        .collect();
    let mut anchor = Server::bind("127.0.0.1:0")
        .unwrap()
        .spawn({
            let received = received.clone();
            move |request: Request| {
                let mut received = received.lock().unwrap();
                received.push(request);
                answers
                    .get(received.len() - 1)
                    .cloned()
                    .unwrap_or_else(|| Response::text(500, "not in the recording"))
            }
        })
        .unwrap();
    let origin = anchor.url();

    let home = tempfile::tempdir().unwrap();
    let challenge = challenge_for(&origin, host).unwrap();
    let mut wallet = Wallet::open(home.path(), &origin).unwrap();
    let mut seeded = StdRng::seed_from_u64(recording["seed"].as_u64().unwrap());
    let refill = wallet
        .refill(credential, &challenge, count, &mut seeded)
        .unwrap();
    anchor.stop();

    // What the client sent is what was recorded.
    let received = received.lock().unwrap();
    assert_eq!(received.len(), exchanges.len());
    for (n, (request, (recorded, _))) in received.iter().zip(&exchanges).enumerate() {
        let mut sent = Vec::new();
        write_request(&mut sent, request).unwrap();
        assert!(
            without_port(&sent) == without_port(recorded),
            "request {} differs from the recording:\n{}",
            n + 1,
            String::from_utf8_lossy(&sent)
        );
        // The unlinkability claim: the Anchor sees neither the edge key nor
        // its name. Checked over every header, recorded or not.
        for name in [
            "Signature",
            "Signature-Input",
            "Signature-Agent",
            "User-Agent",
        ] {
            assert!(
                request.headers.get(name).is_none(),
                "request {} carries {name}",
                n + 1
            );
        }
        // The credential goes to token requests alone.
        let bearer = request.headers.get("Authorization");
        if n == 0 {
            assert_eq!(bearer, None, "the directory is read without the credential");
        } else {
            assert_eq!(bearer, Some(format!("Bearer {credential}").as_str()));
        }
        let recorded = read_request(&mut recorded.as_slice()).unwrap();
        assert_eq!(request.target, recorded.target);
    }

    // The ration as the Anchor stated it, and the key id it signs under.
    let directory: Value = serde_json::from_slice(&answers_body(&exchanges[0].1)).unwrap();
    let token_key = URL_SAFE_NO_PAD
        .decode(directory["token-keys"][0]["token-key"].as_str().unwrap())
        .unwrap();
    let key_id: String = Sha256::digest(&token_key)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    assert_eq!(refill.anchor, origin);
    assert_eq!(refill.origin_info, [host]);
    assert_eq!(refill.requested, count);
    assert_eq!(refill.obtained, 3);
    assert_eq!(refill.held, 3);
    assert_eq!(refill.key_id, key_id);
    assert_eq!(
        refill.stopped,
        Some(Stopped::Refused {
            status: 429,
            retry_after: None,
            stated: json!({"error": "ration exhausted", "ration_per_window": 3}),
        })
    );

    // The wallet: owner-only, its Anchor and key, no credential, and each
    // pass verifying with the key it holds and nothing else.
    let path = wallet.path().to_owned();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600, "the wallet file is owner-only");
        assert_eq!(mode(path.parent().unwrap()), 0o700, "so is its directory");
    }
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains(credential), "the wallet holds no credential");
    let held: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(held["anchor"], origin);
    assert_eq!(held["key_id"], key_id);
    let key = PublicKey::from_spki(
        &URL_SAFE_NO_PAD
            .decode(held["token_key"].as_str().unwrap())
            .unwrap(),
    )
    .unwrap();
    let passes = held["passes"].as_array().unwrap();
    assert_eq!(passes.len(), 3);
    let digest = challenge.digest().unwrap();
    let mut tokens = Vec::new();
    for pass in passes {
        assert_eq!(pass["origin_info"], json!([host]));
        let encoded = pass["token"].as_str().unwrap();
        let bytes = URL_SAFE_NO_PAD.decode(encoded).unwrap();
        let token = PublicToken::tls_deserialize(&mut bytes.as_slice()).unwrap();
        assert_eq!(token.challenge_digest(), &digest);
        assert_eq!(hex(token.token_key_id()), key_id);
        assert!(verifies(&token, &key), "a held pass verifies offline");
        tokens.push(encoded.to_owned());
    }
    // One bit of the nonce, after the two bytes of token type, changed.
    let mut altered = URL_SAFE_NO_PAD.decode(&tokens[0]).unwrap();
    altered[2] ^= 1;
    let altered = PublicToken::tls_deserialize(&mut altered.as_slice()).unwrap();
    assert!(
        !verifies(&altered, &key),
        "the check refuses an altered pass"
    );

    // A pass taken is gone from the file before it is returned.
    let taken = wallet.take(&challenge).unwrap().expect("a pass");
    assert!(verifies(&taken, &key));
    assert_eq!(wallet.held(&challenge).unwrap(), 2);
    let after: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(after["passes"].as_array().unwrap().len(), 2);
    drop(wallet);

    // The event: the Anchor, the counts, the key id and the refusal; never
    // a pass, a blinded request or the credential.
    let mut log = SessionLog::open(home.path(), "replay-session").unwrap();
    log.record_passes_refilled(refill.record()).unwrap();
    let line = std::fs::read_to_string(home.path().join("sessions/replay-session.ndjson")).unwrap();
    let record: Value = serde_json::from_str(line.lines().last().unwrap()).unwrap();
    assert_eq!(record["event"], "passes_refilled");
    let payload = record["payload"].as_object().unwrap();
    let mut keys: Vec<&str> = payload.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "anchor",
            "discarded",
            "held",
            "key_id",
            "obtained",
            "origin_info",
            "requested",
            "session_id",
            "stopped",
            "timestamp"
        ]
    );
    assert_eq!(payload["anchor"], origin);
    assert_eq!(payload["key_id"], key_id);
    assert_eq!(payload["obtained"], 3);
    assert_eq!(payload["discarded"], 0);
    assert_eq!(
        payload["stopped"],
        json!({"status": 429, "retry_after": null,
               "stated": {"error": "ration exhausted", "ration_per_window": 3}})
    );
    assert!(!line.contains(credential));
    for token in &tokens {
        assert!(!line.contains(token.as_str()), "the record carries a pass");
    }
    for (request, _) in &exchanges[1..] {
        let body = read_request(&mut request.as_slice()).unwrap().body;
        for encoding in [URL_SAFE_NO_PAD.encode(&body), hex(&body)] {
            assert!(!line.contains(&encoding), "the record carries a request");
        }
    }
}

fn answers_body(answer: &[u8]) -> Vec<u8> {
    read_response(&mut &answer[..]).unwrap().body
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The stand-in Anchor as recorded, answering each request in turn with
/// the recorded answers named by number.
fn anchor_answering(numbers: &[usize]) -> commonmeasure_http::ServerHandle {
    let exchanges = exchanges();
    let answers: Vec<Response> = numbers
        .iter()
        .map(|n| read_response(&mut exchanges[n - 1].1.as_slice()).expect("a recorded answer"))
        .collect();
    let asked = Arc::new(Mutex::new(0));
    Server::bind("127.0.0.1:0")
        .unwrap()
        .spawn(move |_: Request| {
            let mut asked = asked.lock().unwrap();
            *asked += 1;
            answers
                .get(*asked - 1)
                .cloned()
                .unwrap_or_else(|| Response::text(500, "not in the recording"))
        })
        .unwrap()
}

/// A source's challenge is answered by a pass issued for it under the key
/// the challenge names, and by nothing else; after the Anchor rotates its
/// key, the next refill drops the passes under the old one and says how
/// many it dropped.
#[test]
fn a_pass_is_taken_under_the_key_the_challenge_names_and_a_rotation_drops_the_rest() {
    let recording: Value = serde_json::from_slice(
        &std::fs::read(recorded().join("recording.json")).expect("the recording"),
    )
    .expect("JSON");
    let host = recording["host"].as_str().unwrap();
    let credential = recording["credential"].as_str().unwrap();
    let exchanges = exchanges();

    let mut anchor = anchor_answering(&[1, 2, 3, 4, 5]);
    let origin = anchor.url();
    let home = tempfile::tempdir().unwrap();
    let challenge = challenge_for(&origin, host).unwrap();
    let mut wallet = Wallet::open(home.path(), &origin).unwrap();
    let mut seeded = StdRng::seed_from_u64(recording["seed"].as_u64().unwrap());
    wallet
        .refill(credential, &challenge, 4, &mut seeded)
        .unwrap();
    anchor.stop();

    let directory: Value = serde_json::from_slice(&answers_body(&exchanges[0].1)).unwrap();
    let token_key = URL_SAFE_NO_PAD
        .decode(directory["token-keys"][0]["token-key"].as_str().unwrap())
        .unwrap();
    let header = |key: &[u8]| -> String {
        let (_, value) =
            privacypass::auth::authenticate::build_www_authenticate_header(&challenge, key, None)
                .unwrap();
        value.to_str().unwrap().to_owned()
    };

    // A key the Anchor never issued these passes under: nothing answers.
    let other =
        blind_rsa_signatures::KeyPairSha384PSSDeterministic::generate(&mut seeded, 2048).unwrap();
    let other_key = other.pk.to_spki().unwrap();
    let rotated = source_challenges(&header(&other_key)).unwrap();
    assert_eq!(rotated.len(), 1);
    assert!(rotated[0].names_anchor(&origin) && rotated[0].names_host(host));
    assert_eq!(wallet.held_under_other_keys(&rotated[0]).unwrap(), 3);
    assert!(wallet.take_answering(&rotated[0]).unwrap().is_none());
    assert_eq!(wallet.held(&challenge).unwrap(), 3, "nothing was spent");

    // The key the passes were issued under: one is taken, and it is gone.
    let current = source_challenges(&header(&token_key)).unwrap();
    assert_eq!(wallet.held_under_other_keys(&current[0]).unwrap(), 0);
    assert!(current[0].differs_from_held(&origin, host).is_none());
    let taken = wallet.take_answering(&current[0]).unwrap().expect("a pass");
    let key_id: [u8; 32] = Sha256::digest(&token_key).into();
    assert_eq!(taken.token_key_id(), &key_id);
    assert_eq!(current[0].truncated_token_key_id(), key_id[31]);
    assert_eq!(wallet.held(&challenge).unwrap(), 2);

    // One held pass made to read as issued under another key, as passes
    // from before a rotation do; the next refill reads the directory,
    // drops it, and keeps the two under the current key.
    let path = wallet.path().to_owned();
    drop(wallet);
    let mut file: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let mut bytes = URL_SAFE_NO_PAD
        .decode(file["passes"][0]["token"].as_str().unwrap())
        .unwrap();
    // token_type (2) + nonce (32) + challenge_digest (32), then the key id.
    bytes[2 + 32 + 32] ^= 1;
    let stale = URL_SAFE_NO_PAD.encode(&bytes);
    let mut added = file["passes"][0].clone();
    added["token"] = json!(stale);
    file["passes"].as_array_mut().unwrap().push(added);
    std::fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();

    let mut anchor = anchor_answering(&[1, 5]);
    let origin_now = anchor.url();
    // The wallet is bound to the Anchor's origin, port and all.
    let renamed = std::fs::read_to_string(&path)
        .unwrap()
        .replace(&origin, &origin_now);
    let moved = commonmeasure_harness::passes::wallet_path(home.path(), &origin_now);
    std::fs::write(&moved, renamed).unwrap();
    let mut wallet = Wallet::open(home.path(), &origin_now).unwrap();
    let challenge_now = challenge_for(&origin_now, host).unwrap();
    let refill = wallet
        .refill(credential, &challenge_now, 1, &mut seeded)
        .unwrap();
    anchor.stop();
    assert_eq!(refill.obtained, 0);
    assert_eq!(refill.discarded, 1);
    assert_eq!(refill.held, 2);
    assert_eq!(refill.record()["discarded"], 1);
    let kept: Value = serde_json::from_slice(&std::fs::read(&moved).unwrap()).unwrap();
    assert!(
        kept["passes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|pass| pass["token"] != json!(stale))
    );
}
