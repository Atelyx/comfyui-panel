//! Agent 工具面：把「提交一次生成」注册成宿主 Agent 可调用的工具（`ai.registerTool`）。
//!
//! 调用链：宿主下发 `host:ai.toolCall` → 本模块用当前（或参数指定）的工作流组装草稿并提交 →
//! 经同一 `callId` 以 `ai.toolResult` 把任务 id 或可读失败原因回报给模型。
//! 工具调用在通知 worker 之外起任务驱动：提交要等 ComfyUI 应答，不占住后续通知。
//! 写草稿（提示词覆盖）与面板主提示词框走同一通路，AI 用了什么参数在面板里可见。

use serde_json::{json, Value};

use atelyx_plugin_sdk::protocol::method::ToolCallParams;
use atelyx_plugin_sdk::Context;

use crate::app::App;
use crate::runtime;
use crate::workflow::files::{self, WorkflowFileInfo};
use crate::workflow::form;

/// 工具名：模型可见，全局唯一（不得与内置工具及其他插件重名），字符限字母 / 数字 / `_` / `-`。
pub const TOOL_NAME: &str = "comfy_generate";

/// 工具描述（模型可见）：说清动作、参数与失败形态，避免模型乱猜。
const TOOL_DESCRIPTION: &str = "向本机 ComfyUI 提交一次生成。默认用面板当前选中的工作流；\
可用 workflow 按名字指定工作流（展示名，即下拉里去掉 .json 的名字），用 prompt 覆盖主提示词。\
结果图会出现在 ComfyUI 面板的结果记录流里。未选工作流、指定名匹配不到或提交失败时返回可读错误。";

/// 注册工具：宿主不支持（旧宿主没有 `ai.registerTool`）时静默降级，工具不出现而已。
pub async fn register(ctx: &Context) {
    let parameters = json!({
        "type": "object",
        "properties": {
            "workflow": {
                "type": "string",
                "description": "工作流名（面板下拉里的展示名，不含 .json 后缀）；缺省用当前选中的工作流",
            },
            "prompt": {
                "type": "string",
                "description": "主提示词文本；缺省用面板当前值",
            },
        },
    });
    if let Err(err) = ctx.register_tool(TOOL_NAME, TOOL_DESCRIPTION, parameters).await {
        eprintln!("注册 Agent 工具 {TOOL_NAME} 失败：{err}");
    }
}

/// 处理一次工具调用：另起任务执行，调用结果经同一 `callId` 回报。
pub async fn on_tool_call(app: &std::sync::Arc<App>, params: ToolCallParams) {
    let app = std::sync::Arc::clone(app);
    let call_id = params.call_id.clone();
    tokio::spawn(async move {
        let outcome = run_tool(&app, &params).await;
        let _ = app.ctx.tool_result(&call_id, &outcome.content, outcome.is_error).await;
    });
}

/// 一次工具调用的产物：模型可见文本 + 是否错误。
struct Outcome {
    content: String,
    is_error: bool,
}

impl Outcome {
    fn ok(content: String) -> Self {
        Outcome { content, is_error: false }
    }

    fn error(content: String) -> Self {
        Outcome { content, is_error: true }
    }
}

async fn run_tool(app: &std::sync::Arc<App>, params: &ToolCallParams) -> Outcome {
    let workflow_query = arg_text(&params.arguments, "workflow");
    let prompt_override = arg_text(&params.arguments, "prompt");

    // 指定了工作流就先切过去：加载、转换、草稿重置全走面板同一通路（含参考图缩略回填）
    if let Some(query) = &workflow_query {
        let path = {
            let core = app.core();
            match match_workflow(&core.runtime.snap.workflows, query) {
                Ok(path) => path,
                Err(reason) => return Outcome::error(reason),
            }
        };
        let already = app.core().session.workflow_id.as_deref() == Some(path.as_str());
        if !already && !app.select_workflow_by_path(&path).await {
            return Outcome::error(format!("工作流「{query}」载入失败，请到面板查看原因后重试"));
        }
    }

    // 组装提交内容：提示词覆盖写进面板草稿（用户可见），种子按面板勾选现场重随机
    let (settings, http, client_id, origin_id, prompt, title) = {
        let mut core = app.core();
        if core.session.draft.as_object().map(|map| map.is_empty()).unwrap_or(true) {
            return Outcome::error("尚未选择工作流：请先在 ComfyUI 面板中选择，或在参数里指定 workflow".to_string());
        }
        if let Some(text) = &prompt_override {
            let fields = runtime::build_fields_for(&core.session.draft, &core.runtime.snap);
            match form::resolve_prompt_field(&fields, core.session.prompt_key.as_deref()) {
                Some((field, _)) => {
                    let value = form::coerce_field_value(field, &Value::String(text.clone()));
                    form::write_field(&mut core.session.draft, &field.node_id, &field.input, value);
                }
                None => return Outcome::error("当前工作流没有可覆盖的主提示词字段".to_string()),
            }
        }
        let prompt = form::apply_seed_mode(&core.session.draft, &core.session.seed_mode);
        let title = core.session.workflow_name.clone().unwrap_or_else(|| "手动工作流".to_string());
        (
            core.settings.clone(),
            app.http.clone(),
            core.runtime.book.client_id.clone(),
            core.runtime.book.origin_id.clone(),
            prompt,
            title,
        )
    };
    app.invalidate();

    match runtime::fetch_submit(&settings, &http, &client_id, &origin_id, &prompt).await {
        Ok(prompt_id) => {
            app.core().runtime.apply_submit(&prompt_id, &title);
            app.invalidate();
            Outcome::ok(format!("已提交生成任务 {prompt_id}（工作流「{title}」），结果会出现在面板记录流"))
        }
        Err(err) => Outcome::error(format!("提交失败：{err}")),
    }
}

/// 取字符串参数：只认字符串，空白串按缺省；非字符串类型按缺省（schema 已约束模型）。
fn arg_text(arguments: &Value, key: &str) -> Option<String> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

/// 按展示名匹配工作流：先精确、再忽略大小写；零个或多个命中都给出可读原因（候选列前五个），
/// 不猜一个——选错工作流等于拿用户的图去跑别的流程。
fn match_workflow(files: &[WorkflowFileInfo], query: &str) -> Result<String, String> {
    let display = |file: &WorkflowFileInfo| files::workflow_display_name(&file.path);
    if files.iter().any(|file| display(file) == query) {
        let hits: Vec<&WorkflowFileInfo> = files.iter().filter(|file| display(file) == query).collect();
        if hits.len() > 1 {
            return Err(ambiguous(query, &hits));
        }
        return Ok(hits[0].path.clone());
    }
    let hits: Vec<&WorkflowFileInfo> =
        files.iter().filter(|file| display(file).eq_ignore_ascii_case(query)).collect();
    match hits.len() {
        0 => Err(not_found(query, files)),
        1 => Ok(hits[0].path.clone()),
        _ => Err(ambiguous(query, &hits)),
    }
}

fn not_found(query: &str, files: &[WorkflowFileInfo]) -> String {
    if files.is_empty() {
        return format!("找不到工作流「{query}」：ComfyUI 未返回任何工作流（服务未连接？）");
    }
    let names: Vec<String> = files.iter().take(5).map(|file| files::workflow_display_name(&file.path)).collect();
    format!("找不到工作流「{query}」：现有 {} 个，如{}", files.len(), names.iter().map(|n| format!("「{n}」")).collect::<Vec<_>>().join("、"))
}

fn ambiguous(query: &str, hits: &[&WorkflowFileInfo]) -> String {
    let paths: Vec<String> = hits.iter().take(5).map(|file| file.path.clone()).collect();
    format!("工作流名「{query}」有多个匹配（{}）：{}，请用更具体的名字", hits.len(), paths.join("、"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str) -> WorkflowFileInfo {
        WorkflowFileInfo { path: path.to_string(), name: path.rsplit('/').next().unwrap_or(path).to_string(), modified: 0.0, size: 0.0 }
    }

    #[test]
    fn exact_match_wins_and_resolves_path() {
        let files = vec![file("workflows/猫.json"), file("workflows/狗.json")];
        assert_eq!(match_workflow(&files, "狗"), Ok("workflows/狗.json".to_string()));
    }

    #[test]
    fn case_insensitive_fallback() {
        let files = vec![file("workflows/Portrait.json")];
        assert_eq!(match_workflow(&files, "portrait"), Ok("workflows/Portrait.json".to_string()));
    }

    #[test]
    fn duplicate_display_names_are_ambiguous() {
        let files = vec![file("workflows/a/同款.json"), file("workflows/b/同款.json")];
        let err = match_workflow(&files, "同款").unwrap_err();
        assert!(err.contains("多个匹配"), "{err}");
        assert!(err.contains("workflows/a/同款.json") && err.contains("workflows/b/同款.json"), "候选路径要列出来：{err}");
    }

    #[test]
    fn missing_workflow_lists_candidates() {
        let files = vec![file("workflows/甲.json"), file("workflows/乙.json")];
        let err = match_workflow(&files, "丙").unwrap_err();
        assert!(err.contains("找不到工作流「丙」"), "{err}");
        assert!(err.contains("「甲」"), "候选要列出来：{err}");
    }

    #[test]
    fn empty_list_says_service_may_be_offline() {
        let err = match_workflow(&[], "甲").unwrap_err();
        assert!(err.contains("未返回任何工作流"), "{err}");
    }

    #[test]
    fn exact_match_beats_case_insensitive_collision() {
        // 精确命中唯一时直接用它，不与大小写变体纠缠
        let files = vec![file("workflows/abc.json"), file("workflows/ABC.json")];
        assert_eq!(match_workflow(&files, "abc"), Ok("workflows/abc.json".to_string()));
    }

    #[test]
    fn arg_text_takes_trimmed_non_empty_strings_only() {
        let args = json!({ "workflow": "  同款  ", "prompt": "  ", "count": 3 });
        assert_eq!(arg_text(&args, "workflow"), Some("同款".to_string()));
        assert_eq!(arg_text(&args, "prompt"), None, "空白按缺省");
        assert_eq!(arg_text(&args, "count"), None, "非字符串按缺省");
        assert_eq!(arg_text(&args, "absent"), None);
    }
}
