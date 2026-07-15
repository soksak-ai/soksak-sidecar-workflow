//! soksak-sidecar-workflow — workflow-doc@0.0.1 실행 런타임.
//! 문서를 stage 별로 실행하고(doc_exec), agent 는 claude -p 로 위임한다(provider).
//! 발행 wire = NodeEvent(emit_host), generate 산출 = DraftDoc(draft_doc, validator 인증).

pub mod consensus;
pub mod derive_directive;
pub mod directive_loop;
pub mod doc_exec;
pub mod domain_lib;
pub mod draft_doc;
pub mod emit_host;
pub mod exec_one;
pub mod generate_skeleton;
pub mod host;
pub mod interface;
pub mod lang;
pub mod paths;
pub mod provider;
pub mod reconcile;
pub mod wf_service;
