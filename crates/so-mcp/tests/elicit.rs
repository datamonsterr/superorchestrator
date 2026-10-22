#![allow(clippy::unwrap_used)]
//! `wait_agent` surfaces a child's question to the person through MCP elicitation.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use rmcp::model::{
    CallToolRequestParams, ClientCapabilities, ClientConfig, ElicitRequestParams, ElicitResult,
    ElicitationAction, Implementation,
};
use rmcp::service::{RequestContext, RunningService};
use rmcp::{ClientHandler, ErrorData, RoleClient, ServiceExt};
use serde_json::{Value, json};
use so_core::{Config, Paths};
use so_mcp::McpBridge;
use so_supervisor::{Supervisor, SupervisorClient, SupervisorHandle, SupervisorOptions};
use tempfile::TempDir;

#[derive(Clone)]
struct Person {
    /// `None`: the client cannot elicit.
    reply: Option<ElicitationAction>,
    asked: Arc<Mutex<Vec<String>>>,
}

impl ClientHandler for Person {
    fn get_info(&self) -> ClientConfig {
        let caps = if self.reply.is_some() {
            ClientCapabilities::builder().enable_elicitation().build()
        } else {
            ClientCapabilities::default()
        };
        ClientConfig::new(caps, Implementation::new("person", "1"))
    }

    async fn create_elicitation(
        &self,
        request: ElicitRequestParams,
        _context: RequestContext<RoleClient>,
    ) -> Result<ElicitResult, ErrorData> {
        if let ElicitRequestParams::FormElicitationParams {
            message,
            requested_schema,
            ..
        } = request
        {
            let schema = serde_json::to_string(&requested_schema).unwrap();
            self.asked
                .lock()
                .unwrap()
                .push(format!("{message} | {schema}"));
        }
        let action = self.reply.clone().unwrap_or(ElicitationAction::Decline);
        let r = ElicitResult::new(action.clone());
        Ok(if action == ElicitationAction::Accept {
            r.with_content(json!({"answer": "b"}))
        } else {
            r
        })
    }
}

struct Fixture {
    _dir: TempDir,
    sup: SupervisorHandle,
    client: RunningService<RoleClient, Person>,
    person: Person,
    /// Params of the `send_message` calls the bridge made.
    sent: Arc<Mutex<Vec<Value>>>,
    waits: Arc<Mutex<u32>>,
}

async fn fixture(reply: Option<ElicitationAction>) -> Fixture {
    let dir = tempfile::Builder::new()
        .prefix("somcp-")
        .tempdir_in("/tmp")
        .unwrap();
    let cfg = dir.path().join("cfg");
    std::fs::create_dir_all(&cfg).unwrap();
    let sock = dir.path().join("run/s.sock");
    let sent = Arc::new(Mutex::new(Vec::new()));
    let waits = Arc::new(Mutex::new(0u32));
    let (s2, w2) = (sent.clone(), waits.clone());
    let opts = SupervisorOptions::new(Paths::from_dir(cfg), Config::default(), sock.clone())
        .with_idle_exit(Duration::from_secs(600))
        .with_setup(move |st| {
            let sent = s2.clone();
            st.router.register("send_message", move |_c, p| {
                let sent = sent.clone();
                async move {
                    sent.lock().unwrap().push(p);
                    Ok(json!({"ok": true}))
                }
            });
            let waits = w2.clone();
            st.router.register("wait_agent", move |_c, _p| {
                let waits = waits.clone();
                async move {
                    let n = {
                        let mut w = waits.lock().unwrap();
                        *w += 1;
                        *w
                    };
                    if n == 1 {
                        Ok(json!({"finished": [], "timed_out": false, "updates": [{
                            "id": "e", "session_id": "s", "run_id": "abcdefgh1234",
                            "ts": "2026-01-01T00:00:00Z", "type": "question_asked",
                            "data": {"question": "Which database?", "options": ["a", "b"]}}]}))
                    } else {
                        Ok(json!({"finished": ["abcdefgh1234"], "updates": [], "timed_out": false}))
                    }
                }
            });
        });
    let sup = Supervisor::start(opts).await.unwrap();
    let (sc, _) =
        SupervisorClient::connect_or_spawn_hello(&sock, || Ok(()), "test", "/tmp", None, None)
            .await
            .unwrap();
    let (st, ct) = tokio::io::duplex(65536);
    let bridge = McpBridge::new(sc, false);
    tokio::spawn(async move {
        let svc = bridge.serve(st).await.unwrap();
        let _ = svc.waiting().await;
    });
    let person = Person {
        reply,
        asked: Arc::default(),
    };
    let client = person.clone().serve(ct).await.unwrap();
    Fixture {
        _dir: dir,
        sup,
        client,
        person,
        sent,
        waits,
    }
}

async fn wait(f: &Fixture) -> Value {
    let params = CallToolRequestParams::new("wait_agent").with_arguments(
        json!({"runs": ["abcdefgh1234"], "timeout_ms": 5000})
            .as_object()
            .unwrap()
            .clone(),
    );
    let r = f.client.call_tool(params).await.unwrap();
    serde_json::to_value(&r).unwrap()["structuredContent"].clone()
}

#[tokio::test]
async fn an_answered_question_is_sent_back_and_the_wait_resumes() {
    let f = fixture(Some(ElicitationAction::Accept)).await;
    let r = wait(&f).await;
    assert_eq!(r["finished"][0]["run_id"], "abcdefgh1234");
    let asked = f.person.asked.lock().unwrap().clone();
    assert_eq!(asked.len(), 1);
    assert!(asked[0].contains("Which database?") && asked[0].contains("abcdefgh"));
    assert!(asked[0].contains("\"enum\":[\"a\",\"b\"]"), "{}", asked[0]);
    let sent = f.sent.lock().unwrap().clone();
    assert_eq!(
        sent,
        [json!({"run": "abcdefgh1234", "text": "b", "kind": "answer"})]
    );
    assert_eq!(*f.waits.lock().unwrap(), 2);
    f.sup.shutdown().await;
}

#[tokio::test]
async fn a_declined_question_stays_in_the_updates() {
    let f = fixture(Some(ElicitationAction::Decline)).await;
    let r = wait(&f).await;
    assert_eq!(r["updates"][0]["type"], "question");
    assert!(f.sent.lock().unwrap().is_empty());
    assert_eq!(*f.waits.lock().unwrap(), 1);
    f.sup.shutdown().await;
}

#[tokio::test]
async fn a_client_without_elicitation_gets_the_question() {
    let f = fixture(None).await;
    let r = wait(&f).await;
    assert_eq!(r["updates"][0]["question"], "Which database?");
    assert_eq!(r["updates"][0]["options"], json!(["a", "b"]));
    assert!(f.person.asked.lock().unwrap().is_empty());
    assert!(f.sent.lock().unwrap().is_empty());
    f.sup.shutdown().await;
}
