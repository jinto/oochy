use dioxus::prelude::*;
use kittypaw_core::permission::{PermissionDecision, ResourceKind};

use crate::state::PermissionQueue;

/// Modal overlay that surfaces pending permission requests one at a time.
///
/// Renders nothing when the queue is empty; otherwise shows the *first*
/// request as a centered dialog with three actions:
///   - Allow once (session-only)
///   - Allow permanently (persisted)
///   - Deny
///
/// On any click the entry is removed from the queue and the decision is sent
/// back to the requester via the oneshot channel.
#[component]
pub fn PermissionDialog() -> Element {
    let mut queue = use_context::<PermissionQueue>();

    // Nothing to show.
    if queue.requests.read().is_empty() {
        return rsx! {};
    }

    // Peek at the front request for display (borrow kept short).
    let (kind_label, path, action) = {
        let reqs = queue.requests.read();
        let front = &reqs[0];
        let kind = match front.request.resource_kind {
            ResourceKind::File => "파일",
            ResourceKind::Network => "네트워크",
        };
        (
            kind.to_string(),
            front.request.resource_path.clone(),
            front.request.action.clone(),
        )
    };

    rsx! {
        // ── Backdrop ──
        div {
            style: "
                position: fixed; inset: 0;
                background: rgba(0,0,0,0.45);
                display: flex; align-items: center; justify-content: center;
                z-index: 9999;
            ",

            // ── Dialog card ──
            div {
                style: "
                    background: #FFFFFF;
                    border: 1px solid #E7E5E4;
                    border-radius: 12px;
                    padding: 28px 32px;
                    width: 420px;
                    max-width: 90vw;
                    box-shadow: 0 8px 30px rgba(0,0,0,0.18);
                    font-family: 'Inter', -apple-system, BlinkMacSystemFont, sans-serif;
                ",

                // Header
                h2 {
                    style: "font-size: 17px; font-weight: 600; color: #1C1917; margin: 0 0 18px 0;",
                    "권한 요청"
                }

                // Resource kind
                div { style: "margin-bottom: 10px;",
                    span {
                        style: "font-size: 12px; font-weight: 600; color: #78716C;",
                        "종류"
                    }
                    span {
                        style: "
                            display: inline-block;
                            margin-left: 10px;
                            font-size: 13px;
                            color: #1C1917;
                            background: #F5F3F0;
                            padding: 2px 10px;
                            border-radius: 4px;
                        ",
                        "{kind_label}"
                    }
                }

                // Path
                div { style: "margin-bottom: 10px;",
                    span {
                        style: "font-size: 12px; font-weight: 600; color: #78716C;",
                        "경로"
                    }
                    div {
                        style: "
                            margin-top: 4px;
                            font-size: 13px;
                            font-family: monospace;
                            color: #1C1917;
                            background: #F5F3F0;
                            padding: 6px 10px;
                            border-radius: 4px;
                            word-break: break-all;
                        ",
                        "{path}"
                    }
                }

                // Action
                div { style: "margin-bottom: 22px;",
                    span {
                        style: "font-size: 12px; font-weight: 600; color: #78716C;",
                        "동작"
                    }
                    span {
                        style: "
                            display: inline-block;
                            margin-left: 10px;
                            font-size: 13px;
                            color: #1C1917;
                            background: #F5F3F0;
                            padding: 2px 10px;
                            border-radius: 4px;
                        ",
                        "{action}"
                    }
                }

                // ── Action buttons ──
                div {
                    style: "display: flex; gap: 8px; justify-content: flex-end;",

                    // Deny
                    button {
                        style: "
                            padding: 8px 18px;
                            background: #FFFFFF;
                            color: #DC2626;
                            border: 1px solid #E7E5E4;
                            border-radius: 6px;
                            font-size: 13px;
                            cursor: pointer;
                        ",
                        onclick: move |_| pop_and_respond(&mut queue, PermissionDecision::Deny),
                        "거부"
                    }

                    // Allow once
                    button {
                        style: "
                            padding: 8px 18px;
                            background: #FFFFFF;
                            color: #1C1917;
                            border: 1px solid #E7E5E4;
                            border-radius: 6px;
                            font-size: 13px;
                            cursor: pointer;
                        ",
                        onclick: move |_| pop_and_respond(&mut queue, PermissionDecision::AllowOnce),
                        "허용 (이번만)"
                    }

                    // Allow permanently
                    button {
                        style: "
                            padding: 8px 18px;
                            background: #1C1917;
                            color: #F5F3F0;
                            border: none;
                            border-radius: 6px;
                            font-size: 13px;
                            cursor: pointer;
                        ",
                        onclick: move |_| pop_and_respond(&mut queue, PermissionDecision::AllowPermanent),
                        "항상 허용"
                    }
                }
            }
        }
    }
}

/// Remove the front request from the queue and send the decision back.
fn pop_and_respond(queue: &mut PermissionQueue, decision: PermissionDecision) {
    let mut reqs = queue.requests.write();
    if !reqs.is_empty() {
        let pending = reqs.remove(0);
        let _ = pending.responder.send(decision);
    }
}
