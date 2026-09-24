use serde_json::{json, Value};

const AGENT_ID_PATTERN: &str = "^wc_dagent_[A-Za-z0-9_-]{16}$";
const ENDPOINT_ID_PATTERN: &str = "^wc_endpoint_[A-Za-z0-9_-]{16}$";
const WAIT_ID_PATTERN: &str = "^wc_agent_wait_[A-Za-z0-9_-]{16}$";
const TASK_ID_PATTERN: &str = "^wc_agent_task_[A-Za-z0-9_-]{16}$";

fn id(pattern: &str, description: &str) -> Value {
    json!({"type":"string","pattern":pattern,"description":description})
}

fn idempotency_key(description: &str) -> Value {
    json!({"type":"string","minLength":1,"maxLength":128,"description":description})
}

pub fn wait_for_agent_events_input_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "agent_id": id(AGENT_ID_PATTERN, "Exact caller-owned durable Agent that will resume when this one-shot Wait triggers. Agent identity grants no source-domain authority."),
            "endpoint_id": id(ENDPOINT_ID_PATTERN, "Exact current Agent Endpoint used only as the Host presentation/carrier selector at Wait creation time; it is not persisted as Wait execution ownership."),
            "expected_controller_generation": {"type":"integer","minimum":1,"description":"Exact current Endpoint controller generation. Stale generations fail closed."},
            "events": {
                "type":"array","minItems":1,"maxItems":8,"uniqueItems":true,
                "description":"Closed v1 ANY selector set. Any one matching source fact triggers the one-shot Wait; multiple facts may coalesce only before the durable Host-dispatch fence.",
                "items": {
                    "type":"object","additionalProperties":false,
                    "properties": {
                        "kind": {"type":"string","const":"agent_task_terminal","description":"Durable Agent Wait v1 supports only authoritative AgentTask terminal facts."},
                        "task_id": id(TASK_ID_PATTERN, "Exact independently authorized AgentTask source. This reference grants no Task, Project, Goal, Session, or execution authority.")
                    },
                    "required":["kind","task_id"]
                }
            },
            "idempotency_key": idempotency_key("Caller-generated Wait creation key. Exact replay returns the same Wait; changed reuse conflicts.")
        },
        "required":["agent_id","endpoint_id","expected_controller_generation","events","idempotency_key"]
    })
}

pub fn read_agent_wait_input_schema() -> Value {
    json!({"type":"object","additionalProperties":false,"properties":{"wait_id":id(WAIT_ID_PATTERN,"Exact caller-owned durable AgentWait id. Identity alone grants no authority over its source Tasks.")},"required":["wait_id"]})
}

pub fn cancel_agent_wait_input_schema() -> Value {
    json!({
        "type":"object","additionalProperties":false,
        "properties": {
            "wait_id": id(WAIT_ID_PATTERN,"Exact caller-owned durable AgentWait to cancel before Host dispatch preparation."),
            "idempotency_key": idempotency_key("Caller-generated cancellation key. Exact retry replays; changed reuse conflicts.")
        },
        "required":["wait_id","idempotency_key"]
    })
}

pub fn agent_wait_state_input_schema() -> Value {
    read_agent_wait_input_schema()
}
