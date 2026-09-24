use salvo::prelude::*;
use serde::Deserialize;

use super::{
    prepared, render_communication_result, render_error, require_communication_manage,
    require_communication_read, RuntimeConsoleError,
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CommunicationAgentsInput {
    #[serde(default)]
    agent_id: Option<String>,
    #[serde(default)]
    offset: Option<usize>,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CommunicationAgentCreateInput {
    handle: String,
    display_name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    specialty_labels: Vec<String>,
    idempotency_key: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CommunicationAgentUpdateInput {
    agent_id: String,
    expected_profile_revision: i64,
    #[serde(default)]
    handle: Option<String>,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    specialty_labels: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CommunicationEndpointAttachInput {
    agent_id: String,
    host: String,
    #[serde(default)]
    client_attachment_id: Option<String>,
    idempotency_key: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CommunicationEndpointDetachInput {
    endpoint_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CommunicationEndpointRenewInput {
    endpoint_id: String,
    expected_controller_generation: i64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CommunicationConversationsInput {
    #[serde(default)]
    agent_id: Option<String>,
    #[serde(default)]
    endpoint_id: Option<String>,
    #[serde(default)]
    expected_controller_generation: Option<i64>,
    #[serde(default)]
    offset: Option<usize>,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CommunicationConversationCreateInput {
    #[serde(default)]
    title: Option<String>,
    agent_ids: Vec<String>,
    idempotency_key: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CommunicationConversationInput {
    conversation_id: String,
    #[serde(default)]
    agent_id: Option<String>,
    #[serde(default)]
    endpoint_id: Option<String>,
    #[serde(default)]
    expected_controller_generation: Option<i64>,
    #[serde(default)]
    after_seq: Option<i64>,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CommunicationMessagePostInput {
    conversation_id: String,
    body: String,
    #[serde(default)]
    author_agent_id: Option<String>,
    #[serde(default)]
    endpoint_id: Option<String>,
    #[serde(default)]
    expected_controller_generation: Option<i64>,
    #[serde(default)]
    recipient_agent_ids: Option<Vec<String>>,
    #[serde(default)]
    reply_to: Option<String>,
    #[serde(default)]
    idempotency_key: Option<String>,
    #[serde(default)]
    wake_reply_id: Option<String>,
    #[serde(default)]
    reply_operation_index: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CommunicationInboxInput {
    agent_id: String,
    endpoint_id: String,
    expected_controller_generation: i64,
    #[serde(default)]
    after_delivery_order: Option<i64>,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CommunicationInboxConsumeInput {
    agent_id: String,
    endpoint_id: String,
    expected_controller_generation: i64,
    delivery_ids: Vec<String>,
}

#[handler]
pub(super) async fn communication_agents(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let (runtime, auth) = match prepared(req, depot).await {
        Ok(value) => value,
        Err(error) => return render_error(res, error),
    };
    if let Err(error) = require_communication_read(&auth) {
        return render_error(res, error);
    }
    let input = match req.parse_json::<CommunicationAgentsInput>().await {
        Ok(input) => input,
        Err(_) => return render_error(res, RuntimeConsoleError::Invalid),
    };
    render_communication_result(
        res,
        runtime.list_agent_identities(Some(&auth), input.agent_id, input.offset, input.limit),
    );
}

#[handler]
pub(super) async fn communication_agent_create(
    req: &mut Request,
    depot: &mut Depot,
    res: &mut Response,
) {
    let (runtime, auth) = match prepared(req, depot).await {
        Ok(value) => value,
        Err(error) => return render_error(res, error),
    };
    if let Err(error) = require_communication_manage(&auth) {
        return render_error(res, error);
    }
    let input = match req.parse_json::<CommunicationAgentCreateInput>().await {
        Ok(input) => input,
        Err(_) => return render_error(res, RuntimeConsoleError::Invalid),
    };
    render_communication_result(
        res,
        runtime.create_agent_identity(
            Some(&auth),
            input.handle,
            input.display_name,
            input.description,
            input.specialty_labels,
            input.idempotency_key,
        ),
    );
}

#[handler]
pub(super) async fn communication_agent_update(
    req: &mut Request,
    depot: &mut Depot,
    res: &mut Response,
) {
    let (runtime, auth) = match prepared(req, depot).await {
        Ok(value) => value,
        Err(error) => return render_error(res, error),
    };
    if let Err(error) = require_communication_manage(&auth) {
        return render_error(res, error);
    }
    let input = match req.parse_json::<CommunicationAgentUpdateInput>().await {
        Ok(input) => input,
        Err(_) => return render_error(res, RuntimeConsoleError::Invalid),
    };
    render_communication_result(
        res,
        runtime.update_agent_identity(
            Some(&auth),
            input.agent_id,
            input.expected_profile_revision,
            input.handle,
            input.display_name,
            input.description,
            input.specialty_labels,
        ),
    );
}

#[handler]
pub(super) async fn communication_endpoint_attach(
    req: &mut Request,
    depot: &mut Depot,
    res: &mut Response,
) {
    let (runtime, auth) = match prepared(req, depot).await {
        Ok(value) => value,
        Err(error) => return render_error(res, error),
    };
    if let Err(error) = require_communication_manage(&auth) {
        return render_error(res, error);
    }
    let input = match req.parse_json::<CommunicationEndpointAttachInput>().await {
        Ok(input) => input,
        Err(_) => return render_error(res, RuntimeConsoleError::Invalid),
    };
    render_communication_result(
        res,
        runtime.attach_agent_endpoint(
            Some(&auth),
            input.agent_id,
            input.host,
            input.client_attachment_id,
            input.idempotency_key,
        ),
    );
}

#[handler]
pub(super) async fn communication_endpoint_renew(
    req: &mut Request,
    depot: &mut Depot,
    res: &mut Response,
) {
    let (runtime, auth) = match prepared(req, depot).await {
        Ok(value) => value,
        Err(error) => return render_error(res, error),
    };
    if let Err(error) = require_communication_manage(&auth) {
        return render_error(res, error);
    }
    let input = match req.parse_json::<CommunicationEndpointRenewInput>().await {
        Ok(input) => input,
        Err(_) => return render_error(res, RuntimeConsoleError::Invalid),
    };
    render_communication_result(
        res,
        runtime.renew_agent_endpoint(
            Some(&auth),
            input.endpoint_id,
            input.expected_controller_generation,
        ),
    );
}

#[handler]
pub(super) async fn communication_endpoint_detach(
    req: &mut Request,
    depot: &mut Depot,
    res: &mut Response,
) {
    let (runtime, auth) = match prepared(req, depot).await {
        Ok(value) => value,
        Err(error) => return render_error(res, error),
    };
    if let Err(error) = require_communication_manage(&auth) {
        return render_error(res, error);
    }
    let input = match req.parse_json::<CommunicationEndpointDetachInput>().await {
        Ok(input) => input,
        Err(_) => return render_error(res, RuntimeConsoleError::Invalid),
    };
    render_communication_result(
        res,
        runtime.detach_agent_endpoint(Some(&auth), input.endpoint_id),
    );
}

#[handler]
pub(super) async fn communication_conversations(
    req: &mut Request,
    depot: &mut Depot,
    res: &mut Response,
) {
    let (runtime, auth) = match prepared(req, depot).await {
        Ok(value) => value,
        Err(error) => return render_error(res, error),
    };
    if let Err(error) = require_communication_read(&auth) {
        return render_error(res, error);
    }
    let input = match req.parse_json::<CommunicationConversationsInput>().await {
        Ok(input) => input,
        Err(_) => return render_error(res, RuntimeConsoleError::Invalid),
    };
    render_communication_result(
        res,
        runtime.list_conversations(
            Some(&auth),
            input.agent_id,
            input.endpoint_id,
            input.expected_controller_generation,
            input.offset,
            input.limit,
        ),
    );
}

#[handler]
pub(super) async fn communication_conversation_create(
    req: &mut Request,
    depot: &mut Depot,
    res: &mut Response,
) {
    let (runtime, auth) = match prepared(req, depot).await {
        Ok(value) => value,
        Err(error) => return render_error(res, error),
    };
    if let Err(error) = require_communication_manage(&auth) {
        return render_error(res, error);
    }
    let input = match req
        .parse_json::<CommunicationConversationCreateInput>()
        .await
    {
        Ok(input) => input,
        Err(_) => return render_error(res, RuntimeConsoleError::Invalid),
    };
    render_communication_result(
        res,
        runtime.create_conversation(
            Some(&auth),
            input.title,
            input.agent_ids,
            input.idempotency_key,
        ),
    );
}

#[handler]
pub(super) async fn communication_conversation(
    req: &mut Request,
    depot: &mut Depot,
    res: &mut Response,
) {
    let (runtime, auth) = match prepared(req, depot).await {
        Ok(value) => value,
        Err(error) => return render_error(res, error),
    };
    if let Err(error) = require_communication_read(&auth) {
        return render_error(res, error);
    }
    let input = match req.parse_json::<CommunicationConversationInput>().await {
        Ok(input) => input,
        Err(_) => return render_error(res, RuntimeConsoleError::Invalid),
    };
    render_communication_result(
        res,
        runtime.read_conversation(
            Some(&auth),
            input.conversation_id,
            input.agent_id,
            input.endpoint_id,
            input.expected_controller_generation,
            input.after_seq,
            input.limit,
        ),
    );
}

#[handler]
pub(super) async fn communication_message_post(
    req: &mut Request,
    depot: &mut Depot,
    res: &mut Response,
) {
    let (runtime, auth) = match prepared(req, depot).await {
        Ok(value) => value,
        Err(error) => return render_error(res, error),
    };
    if let Err(error) = require_communication_manage(&auth) {
        return render_error(res, error);
    }
    let input = match req.parse_json::<CommunicationMessagePostInput>().await {
        Ok(input) => input,
        Err(_) => return render_error(res, RuntimeConsoleError::Invalid),
    };
    render_communication_result(
        res,
        runtime.post_conversation_message(
            Some(&auth),
            input.conversation_id,
            input.body,
            input.author_agent_id,
            input.endpoint_id,
            input.expected_controller_generation,
            input.recipient_agent_ids,
            input.reply_to,
            input.idempotency_key,
            input.wake_reply_id,
            input.reply_operation_index,
        ),
    );
}

#[handler]
pub(super) async fn communication_inbox(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let (runtime, auth) = match prepared(req, depot).await {
        Ok(value) => value,
        Err(error) => return render_error(res, error),
    };
    if let Err(error) = require_communication_read(&auth) {
        return render_error(res, error);
    }
    let input = match req.parse_json::<CommunicationInboxInput>().await {
        Ok(input) => input,
        Err(_) => return render_error(res, RuntimeConsoleError::Invalid),
    };
    render_communication_result(
        res,
        runtime.list_agent_inbox(
            Some(&auth),
            input.agent_id,
            input.endpoint_id,
            input.expected_controller_generation,
            input.after_delivery_order,
            input.limit,
        ),
    );
}

#[handler]
pub(super) async fn communication_inbox_consume(
    req: &mut Request,
    depot: &mut Depot,
    res: &mut Response,
) {
    let (runtime, auth) = match prepared(req, depot).await {
        Ok(value) => value,
        Err(error) => return render_error(res, error),
    };
    if let Err(error) = require_communication_manage(&auth) {
        return render_error(res, error);
    }
    let input = match req.parse_json::<CommunicationInboxConsumeInput>().await {
        Ok(input) => input,
        Err(_) => return render_error(res, RuntimeConsoleError::Invalid),
    };
    render_communication_result(
        res,
        runtime.consume_agent_deliveries(
            Some(&auth),
            input.agent_id,
            input.endpoint_id,
            input.expected_controller_generation,
            input.delivery_ids,
        ),
    );
}
