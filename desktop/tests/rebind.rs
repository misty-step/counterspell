//! Rebind exercised against a FAKE herdr control socket — never a live pane.
//! Proves the write surface the desktop app's `rebind_pane` command drives
//! (`api::rebind_pane` → `pane.report_agent_session` over the herdr socket)
//! sends exactly the report a broken binding needs, and round-trips the herdr
//! response back into a `reported: true` outcome.
//!
//! A scripted stub stands in for herdr the way `control.rs` stands isolated
//! dirs in for the real marker/config: no live herdr, no real pane mutated.
//!
//! One `#[test]` on purpose: it sets a process-global env override
//! (`COUNTERSPELL_HERDR_SOCKET`), so a single test per binary avoids env races.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

#[test]
fn rebind_reports_agent_session_to_a_fake_herdr_socket() {
    let temp = tempfile::tempdir().expect("tempdir");
    let socket_path = temp.path().join("herdr.sock");
    let listener = UnixListener::bind(&socket_path).expect("bind fake herdr socket");

    // Fake herdr: accept one connection, capture the report request, answer
    // with a JSON-RPC-shaped ok result (a non-empty line == reported).
    let (tx, rx) = mpsc::channel::<serde_json::Value>();
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().expect("accept rebind connection");
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).expect("read request line");
        let request: serde_json::Value = serde_json::from_str(line.trim()).expect("parse request");
        let id = request
            .get("id")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let mut stream = reader.into_inner();
        let response = serde_json::json!({ "id": id, "result": { "ok": true } });
        let mut bytes = serde_json::to_vec(&response).expect("encode response");
        bytes.push(b'\n');
        stream.write_all(&bytes).expect("write response");
        tx.send(request).expect("hand request back to test");
    });

    // Safe: the only test in this binary; env is process-global but uncontended.
    std::env::set_var("COUNTERSPELL_HERDR_SOCKET", &socket_path);

    let pane_id = "w49:p2";
    let session_id = "sess-rebind-abc123";
    let outcome =
        counterspell::api::rebind_pane(pane_id, session_id, None).expect("rebind should report");

    std::env::remove_var("COUNTERSPELL_HERDR_SOCKET");

    // The command round-trips the herdr response into a reported outcome.
    assert!(
        outcome.reported,
        "a non-empty herdr response means reported"
    );
    assert_eq!(outcome.pane_id, pane_id);
    assert_eq!(outcome.session_id, session_id);

    // The report herdr received is a real `pane.report_agent_session` carrying
    // exactly the pane + session the window asked to rebind.
    let request = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("fake herdr received a request");
    assert_eq!(request["method"], "pane.report_agent_session");
    assert_eq!(request["params"]["pane_id"], pane_id);
    assert_eq!(request["params"]["agent_session_id"], session_id);
    assert_eq!(request["params"]["agent"], "claude");

    server.join().expect("server thread");
}
