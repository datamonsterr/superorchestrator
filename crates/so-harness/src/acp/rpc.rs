use crate::{driver::HarnessEvent, process::LineParser};
use serde_json::Value;
use tokio::sync::mpsc;

pub struct Parser(pub mpsc::UnboundedSender<Value>);
impl LineParser for Parser {
    fn parse_line(&mut self, line: &str) -> Vec<HarnessEvent> {
        if let Ok(v) = serde_json::from_str(line) {
            let _ = self.0.send(v);
        }
        vec![]
    }
}
