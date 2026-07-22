//! soksak-sidecar-workflow — workflow-doc@0.0.1 실행 런타임.
//! 문서를 stage 별로 실행하고(doc_interp), agent 는 claude -p 로 위임한다(provider).
//! 발행 wire = NodeEvent(node_event).

pub mod author_doc;
pub mod consensus;
pub mod derive_directive;
pub mod doc_interp;
pub mod domain_lib;
pub mod exec_one;
pub mod interface;
pub mod lang;
pub mod mem_board;
pub mod node_event;
pub mod paths;
pub mod prompt_assembly;
pub mod provider;
pub mod reconcile;
pub mod spec_set;
pub mod wf_service;
