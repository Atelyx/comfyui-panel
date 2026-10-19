//! 生成运行时：连接探测、队列/历史轮询、结果折叠与副本抓取、提交/中断/释放、WS 事件落地。
//!
//! 纯状态机：所有网络与磁盘 IO 收在自由 `fetch_*` 函数里（不碰 `Runtime`，可安全 `await`），
//! 拿回数据后由同步 `apply_*` 方法落到快照。调用方（app）据此把「读输入 → 释放锁 → await IO →
//! 重新加锁写回」串起来，绝不在持有状态锁时跨 `await`，避免慢 IO 卡住整棵界面重绘。

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::mem;

use serde_json::Value;

use crate::comfy::client::ComfyClient;
use crate::comfy::types::{
    create_time_of, history_failed, history_outputs, history_prompt, origin_of, output_refs,
    queue_item_extra, queue_item_prompt, queue_item_prompt_id, ExecutingPayload,
    ExecutionErrorPayload, ImageRef, ProgressPayload,
};
use crate::history::{
    copy_name, copy_path, retry_copy_name, CopyStore, ResultImage, HISTORY_LIMIT, RECAPTURE_ATTEMPTS,
};
use crate::settings::{base_url, ComfySettings};
use crate::workflow::files::{diff_workflows, WorkflowFileInfo};
use crate::workflow::form::{prompt_text_of, FieldSpec};
use crate::workflow::library;

/// 每次历史拉取的最大条数（够覆盖重启前后未折叠的任务即可）。
const HISTORY_BOOTSTRAP: u32 = 30;

/// 连接通道：直连成功 / 断开。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    Direct,
    Offline,
}

/// 当前仓库类型：仅个人仓库（Local）可落库；协作空间（Space）与尚未收到切换事件的未知态都禁用
/// ——落库需要仓库根路径，未知/协作空间下根路径为空，物理上无法写入。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VaultKind {
    Local,
    Space,
    Unknown,
}

/// 面板消费的完整快照。
#[derive(Clone, Debug)]
pub struct RuntimeSnapshot {
    pub channel: Channel,
    pub vault_kind: VaultKind,
    pub probing: bool,
    pub system_stats: Option<Value>,
    pub queue_running: Vec<Value>,
    pub queue_pending: Vec<Value>,
    pub progress: Option<ProgressPayload>,
    pub running_prompt_id: Option<String>,
    pub error: String,
    pub results: Vec<ResultImage>,
    pub object_info: Option<Value>,
    pub workflows: Vec<WorkflowFileInfo>,
    pub workflows_error: String,
}

impl Default for RuntimeSnapshot {
    fn default() -> Self {
        RuntimeSnapshot {
            channel: Channel::Offline,
            vault_kind: VaultKind::Unknown,
            probing: false,
            system_stats: None,
            queue_running: Vec::new(),
            queue_pending: Vec::new(),
            progress: None,
            running_prompt_id: None,
            error: String::new(),
            results: Vec::new(),
            object_info: None,
            workflows: Vec::new(),
            workflows_error: String::new(),
        }
    }
}

/// 不进快照、仅供运行时内部去重与配对的记账。
#[derive(Default)]
pub struct Bookkeeping {
    /// 已折叠进画廊的 prompt id：防轮询重复摄入（含被他机摄入的）。
    pub ingested_prompts: BTreeSet<String>,
    /// 提交时记录的标题，等历史条目到达时配对（历史里不一定带标题）。
    pub titles: BTreeMap<String, String>,
    /// 本机来源标记：写进 extra_data，跨机时用于过滤他机任务。
    pub origin_id: String,
    /// 提交与 WS 共用的客户端 id（服务端据此把进度推回本面板）。
    pub client_id: String,
}

/// 运行时聚合体：只持状态，不做 IO。
pub struct Runtime {
    pub snap: RuntimeSnapshot,
    pub book: Bookkeeping,
}

impl Runtime {
    pub fn new() -> Self {
        Runtime { snap: RuntimeSnapshot::default(), book: Bookkeeping { client_id: random_hex(16), ..Default::default() } }
    }

    // ---------- 连接 ----------

    /// 探测态：进入周期时置真、拿到通道后由 [`apply_probe`] 落定；界面据此显示「连接中」。
    pub fn set_probing(&mut self, on: bool) {
        self.snap.probing = on;
    }

    pub fn apply_probe(&mut self, channel: Channel) {
        self.snap.channel = channel;
        self.snap.probing = false;
    }

    /// 断开：清通道、进度与队列/统计/节点定义（保留画廊与副本，重连后重新拉取）。
    pub fn disconnect(&mut self) {
        self.snap.channel = Channel::Offline;
        self.snap.progress = None;
        self.snap.running_prompt_id = None;
        self.clear_server_cache();
    }

    /// 作废来自服务端的易变缓存：队列与统计属于上一次连接，节点定义会随服务重启或换地址失效。
    fn clear_server_cache(&mut self) {
        self.snap.queue_running.clear();
        self.snap.queue_pending.clear();
        self.snap.system_stats = None;
        self.snap.object_info = None;
    }

    pub fn clear_error(&mut self) {
        self.snap.error.clear();
    }

    /// 载入持久化的 originId 与历史：先于连接执行，历史里的 prompt id 入去重集防 bootstrap 重复摄入。
    pub fn load_persisted(&mut self, origin_id: String, history: Vec<ResultImage>) {
        if !origin_id.is_empty() {
            self.book.origin_id = origin_id;
        } else {
            self.book.origin_id = random_hex(8);
        }
        for item in &history {
            self.book.ingested_prompts.insert(item.prompt_id.clone());
        }
        self.snap.results = history;
    }

    // ---------- 只读刷新的落地点（数据由对应 fetch_* 备好）----------

    pub fn apply_system_stats(&mut self, result: Result<Value, String>) {
        match result {
            Ok(stats) => self.snap.system_stats = Some(stats),
            Err(err) => self.snap.error = err,
        }
    }

    pub fn apply_queue(&mut self, queue: &Value) {
        let origin = self.book.origin_id.clone();
        self.snap.queue_running = filter_queue(queue, "queue_running", &origin);
        self.snap.queue_pending = filter_queue(queue, "queue_pending", &origin);
    }

    pub fn apply_object_info(&mut self, info: Option<Value>) {
        if let Some(info) = info {
            self.snap.object_info = Some(info);
        }
    }

    /// 落工作流清单，返回画廊之外的清单/错误是否变化（供上层判断当前工作流被外部改过或需清错）。
    pub fn apply_workflows(&mut self, result: Result<Vec<WorkflowFileInfo>, String>) -> bool {
        match result {
            Ok(files) => {
                let previous: HashMap<String, WorkflowFileInfo> =
                    self.snap.workflows.iter().map(|f| (f.path.clone(), f.clone())).collect();
                let (changed, removed) = diff_workflows(&previous, &files);
                let untouched = changed.is_empty() && removed.is_empty() && files.len() == previous.len();
                if untouched {
                    if !self.snap.workflows_error.is_empty() {
                        self.snap.workflows_error.clear();
                        return true;
                    }
                    false
                } else {
                    self.snap.workflows = files;
                    self.snap.workflows_error.clear();
                    true
                }
            }
            Err(err) => {
                if err != self.snap.workflows_error {
                    self.snap.workflows_error = err;
                    return true;
                }
                false
            }
        }
    }

    // ---------- 历史折叠：取新 → 抓副本（IO）→ 并入 ----------

    /// 从历史对象里取出「本机、未摄入」的新结果项（不含副本），并把其 prompt id 记入去重集。
    /// 去重集同步落定，并发轮询各自取到的 fresh 不重叠，故无重复入库；他机任务登记去重但不展示。
    pub fn take_fresh_results(&mut self, history: &Value) -> Vec<ResultImage> {
        let Some(map) = history.as_object() else {
            return Vec::new();
        };
        let origin = self.book.origin_id.clone();
        let mut fresh = Vec::new();
        for (prompt_id, entry) in map {
            if self.book.ingested_prompts.contains(prompt_id) {
                continue;
            }
            self.book.ingested_prompts.insert(prompt_id.clone());
            let entry_origin = origin_of(history_prompt(entry).and_then(queue_item_extra));
            if entry_origin.is_some() && entry_origin.as_deref() != Some(origin.as_str()) {
                continue;
            }
            let title = self.book.titles.get(prompt_id).cloned().unwrap_or_default();
            fresh.extend(images_of(prompt_id, entry, &title));
        }
        fresh
    }

    /// 新结果并入画廊：同 key 只保留画廊里已有的那条、按时间倒序、超上限淘汰并删其副本；返回是否真的并入了内容。
    pub fn merge_results(&mut self, fresh: Vec<ResultImage>, store: &CopyStore) -> bool {
        if fresh.is_empty() {
            return false;
        }
        let mut all = mem::take(&mut self.snap.results);
        all.extend(fresh);
        // 同一 key 就是同一张源图，留两份会让淘汰时删掉另一份仍在引用的副本。
        // 排序前先去重：画廊原条目在前，因此保留的是已带副本路径与入库记录的那一份。
        let mut seen = BTreeSet::new();
        all.retain(|item| seen.insert(item.key.clone()));
        all.sort_by(|a, b| b.created_at.partial_cmp(&a.created_at).unwrap_or(std::cmp::Ordering::Equal));
        if all.len() > HISTORY_LIMIT {
            let evicted = all.split_off(HISTORY_LIMIT);
            for item in evicted {
                if let Some(local) = item.local {
                    store.remove(&local);
                }
            }
        }
        self.snap.results = all;
        true
    }

    /// 取仍缺本地副本、且没耗尽补抓轮次的结果（首抓遇网络/写盘失败留下的缺口），供补抓重试。
    /// 快照按时间倒序，`limit` 条即用户最可能看到的最近缺口。
    pub fn take_uncopied(&self, limit: usize) -> Vec<ResultImage> {
        self.snap.results.iter()
            .filter(|item| item.local.is_none() && item.copy_attempts < RECAPTURE_ATTEMPTS)
            .take(limit)
            .cloned()
            .collect()
    }

    /// 落补抓结果：成功的回填 `local`，仍缺副本的累计一轮尝试；返回回填条数。
    pub fn apply_recapture(&mut self, done: &[ResultImage]) -> usize {
        let mut filled = 0;
        for item in done {
            for slot in self.snap.results.iter_mut() {
                if slot.key != item.key || slot.local.is_some() {
                    continue;
                }
                match &item.local {
                    Some(path) => {
                        slot.local = Some(path.clone());
                        filled += 1;
                    }
                    None => slot.copy_attempts += 1,
                }
                break;
            }
        }
        filled
    }

    // ---------- 提交与控制 ----------

    /// 提交成功后记录标题、回填先到的同名任务、置为运行中。
    pub fn apply_submit(&mut self, prompt_id: &str, title: &str) {
        self.book.titles.insert(prompt_id.to_string(), title.to_string());
        for item in self.snap.results.iter_mut() {
            if item.prompt_id == prompt_id && item.title != title {
                item.title = title.to_string();
            }
        }
        self.snap.running_prompt_id = Some(prompt_id.to_string());
        self.snap.error.clear();
        self.snap.progress = None;
    }

    /// 中断目标：优先运行中任务，其次队首。
    pub fn interrupt_target(&self) -> Option<String> {
        self.snap.running_prompt_id.clone().or_else(|| {
            self.snap.queue_running.first().and_then(|item| queue_item_prompt_id(item).map(str::to_string))
        })
    }

    pub fn apply_interrupt(&mut self) {
        self.snap.progress = None;
    }

    /// 标记录入仓库路径。
    pub fn mark_saved(&mut self, key: &str, saved_path: String) {
        for item in self.snap.results.iter_mut() {
            if item.key == key {
                item.saved_path = Some(saved_path.clone());
            }
        }
    }

    // ---------- WebSocket 事件落地（副作用刷新由 app 依返回值决定）----------

    pub fn apply_ws_progress(&mut self, progress: ProgressPayload) {
        self.snap.running_prompt_id = progress.prompt_id.clone().or_else(|| self.snap.running_prompt_id.clone());
        self.snap.progress = Some(progress);
    }

    /// 执行节点更新；返回 `true` 表示本轮结束（node 为空），app 据此刷新队列与历史。
    pub fn apply_ws_executing(&mut self, executing: ExecutingPayload) -> bool {
        if executing.node.is_some() {
            self.snap.running_prompt_id = executing.prompt_id.or_else(|| self.snap.running_prompt_id.clone());
            false
        } else {
            self.apply_execution_end()
        }
    }

    /// 本轮执行收场：清掉进度与运行中标记；返回是否真的有状态变化（无变化时上层不必刷新）。
    /// 结束信号有三种发法（空节点的 executing、execution_success、execution_interrupted），
    /// 重复的收场事件因此不会触发第二次刷新。
    pub fn apply_execution_end(&mut self) -> bool {
        let changed = self.snap.progress.is_some() || self.snap.running_prompt_id.is_some();
        self.snap.progress = None;
        self.snap.running_prompt_id = None;
        changed
    }

    pub fn apply_ws_error(&mut self, error: ExecutionErrorPayload) {
        let subject = match &error.node_type {
            Some(node_type) => format!("节点 {node_type} 出错"),
            None => "执行出错".to_string(),
        };
        let message = error.exception_message.unwrap_or_else(|| "未知错误".to_string());
        self.snap.error = format!("{subject}：{message}");
        self.snap.progress = None;
    }
}

// ---------- 网络/磁盘 IO：全部自由函数，不触碰 Runtime ----------

fn client_for(settings: &ComfySettings, http: &reqwest::Client) -> ComfyClient {
    ComfyClient::new(base_url(settings), http.clone())
}

pub async fn fetch_probe(settings: &ComfySettings, http: &reqwest::Client) -> Channel {
    if client_for(settings, http).probe().await {
        Channel::Direct
    } else {
        Channel::Offline
    }
}

pub async fn fetch_system_stats(settings: &ComfySettings, http: &reqwest::Client) -> Result<Value, String> {
    client_for(settings, http).system_stats().await
}

pub async fn fetch_queue(settings: &ComfySettings, http: &reqwest::Client) -> Result<Value, String> {
    client_for(settings, http).queue().await
}

/// 拉历史；失败返回 Err（调用方静默：通道字段已表达连接存活）。
pub async fn fetch_history(settings: &ComfySettings, http: &reqwest::Client) -> Result<Value, String> {
    client_for(settings, http).history(HISTORY_BOOTSTRAP).await
}

/// 拉节点定义；失败返回 None（表单退化为按值类型的原始编辑）。
pub async fn fetch_object_info(settings: &ComfySettings, http: &reqwest::Client) -> Option<Value> {
    client_for(settings, http).object_info().await.ok()
}

pub async fn fetch_workflows(settings: &ComfySettings, http: &reqwest::Client) -> Result<Vec<WorkflowFileInfo>, String> {
    library::list_workflows(&client_for(settings, http)).await
}

/// 提交并取出 prompt id；服务端未回 id 时把 `error.message` 或兜底原因作为 Err。
pub async fn fetch_submit(
    settings: &ComfySettings,
    http: &reqwest::Client,
    client_id: &str,
    origin_id: &str,
    prompt: &Value,
) -> Result<String, String> {
    let result = client_for(settings, http).submit(prompt, client_id, origin_id).await?;
    result
        .get("prompt_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            result
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| "提交失败：服务端未返回任务 id".to_string())
        })
}

/// 定向中断；失败静默（中断本就尽力而为）。
pub async fn fetch_interrupt(settings: &ComfySettings, http: &reqwest::Client, prompt_id: &str) {
    let _ = client_for(settings, http).interrupt_prompt(prompt_id).await;
}

pub async fn fetch_free(settings: &ComfySettings, http: &reqwest::Client) -> Result<(), String> {
    client_for(settings, http).free(true, true).await
}

pub async fn fetch_upload(settings: &ComfySettings, http: &reqwest::Client, filename: &str, bytes: Vec<u8>) -> Result<ImageRef, String> {
    client_for(settings, http).upload_image(filename, bytes).await
}

pub async fn fetch_image_bytes(settings: &ComfySettings, http: &reqwest::Client, image_ref: &ImageRef) -> Result<Vec<u8>, String> {
    client_for(settings, http).image_bytes(image_ref).await
}

/// WebSocket 地址：`client_id` 与提交共用，服务端据此把进度推回本面板。
pub fn socket_url(settings: &ComfySettings, client_id: &str, http: &reqwest::Client) -> String {
    client_for(settings, http).socket_url(client_id)
}

/// 立即抓取新结果的字节副本并回填 `local`（预览节点输出会随服务重启消失，须当场落盘）。
pub async fn capture_copies(fresh: &mut [ResultImage], settings: &ComfySettings, http: &reqwest::Client, store: &CopyStore) {
    let client = client_for(settings, http);
    for (index, item) in fresh.iter_mut().enumerate() {
        let path = copy_path(&copy_name(&item.prompt_id, index, &item.image_ref.filename));
        if let Ok(bytes) = client.image_bytes(&item.image_ref).await {
            if store.save(&path, &bytes).is_ok() {
                item.local = Some(path);
            }
        }
    }
}

/// 补抓仍缺副本的结果：副本名用 `retry-` 前缀（[`retry_copy_name`]）与首次抓取的 index 命名区分，
/// 故多次补抓幂等、且绝不覆盖已成功的副本。抓不到的（源已被服务端回收）保持 `local` 为空。
pub async fn recapture_copies(pending: &mut [ResultImage], settings: &ComfySettings, http: &reqwest::Client, store: &CopyStore) {
    let client = client_for(settings, http);
    for item in pending.iter_mut() {
        let path = copy_path(&retry_copy_name(&item.prompt_id, &item.image_ref.filename));
        if let Ok(bytes) = client.image_bytes(&item.image_ref).await {
            if store.save(&path, &bytes).is_ok() {
                item.local = Some(path);
            }
        }
    }
}

// ---------- 载入/存回工作流（读写用户目录文件）----------

pub async fn load_workflow(
    settings: &ComfySettings,
    http: &reqwest::Client,
    file: &WorkflowFileInfo,
) -> Result<library::StoredWorkflow, String> {
    library::load_workflow(&client_for(settings, http), file).await
}

pub async fn save_workflow(
    settings: &ComfySettings,
    http: &reqwest::Client,
    file: &WorkflowFileInfo,
    prompt: &Value,
) -> Result<(), String> {
    library::save_workflow_params(&client_for(settings, http), file, prompt).await
}

// ---------- 纯数据辅助（无 IO）----------

/// 从一条历史条目展开结果项（仅 images + gifs，其余输出忽略）。
fn images_of(prompt_id: &str, entry: &Value, submitted_title: &str) -> Vec<ResultImage> {
    let outputs = history_outputs(entry).and_then(Value::as_object).cloned().unwrap_or_default();
    let failed = history_failed(entry);
    let extra = history_prompt(entry).and_then(queue_item_extra);
    let created_at = create_time_of(extra).unwrap_or_else(now_millis);
    let prompt = history_prompt(entry).and_then(queue_item_prompt);
    let prompt_text = prompt_text_of(prompt.unwrap_or(&Value::Null));
    let mut items = Vec::new();
    for output in outputs.values() {
        for image_ref in output_refs(output) {
            let key = format!("{}:{}/{}", prompt_id, image_ref.subfolder, image_ref.filename);
            let title = if !submitted_title.is_empty() {
                submitted_title.to_string()
            } else {
                prompt_title_fallback(entry)
            };
            items.push(ResultImage {
                key,
                prompt_id: prompt_id.to_string(),
                image_ref,
                title,
                prompt_text: prompt_text.clone(),
                created_at,
                failed,
                saved_path: None,
                local: None,
                copy_attempts: 0,
            });
        }
    }
    items
}

/// 历史条目里没有提交标题时，取第一个带非空 `_meta.title` 的节点名兜底。
fn prompt_title_fallback(entry: &Value) -> String {
    let fallback = "未命名任务";
    let Some(prompt) = history_prompt(entry).and_then(queue_item_prompt).and_then(Value::as_object) else {
        return fallback.to_string();
    };
    for node in prompt.values() {
        if let Some(title) = node
            .get("_meta")
            .and_then(|m| m.get("title"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|t| !t.is_empty())
        {
            return title.to_string();
        }
    }
    fallback.to_string()
}

/// 按来源过滤队列：无标记（其他客户端）或本机标记的才显示。
fn filter_queue(queue: &Value, field: &str, origin_id: &str) -> Vec<Value> {
    queue
        .get(field)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter(|item| match origin_of(queue_item_extra(item)) {
                    None => true,
                    Some(o) => o == origin_id,
                })
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// 当前毫秒时间戳（与 ComfyUI create_time 同单位）。
fn now_millis() -> f64 {
    chrono::Utc::now().timestamp_millis() as f64
}

/// 系统熵生成小写十六进制串（`byte_len` 字节 → 2×`byte_len` 字符）。
fn random_hex(byte_len: usize) -> String {
    let mut bytes = vec![0u8; byte_len];
    if getrandom::getrandom(&mut bytes).is_err() {
        bytes.fill(0);
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// 供 app 组装草稿字段时读取当前对象定义（薄封装，集中在此以免视图层到处取）。
pub fn object_info_of(snap: &RuntimeSnapshot) -> Option<&Value> {
    snap.object_info.as_ref()
}

/// 字段列表：从草稿 + 定义推导（视图层调用，收口在此以免散落到 views）。
pub fn build_fields_for(prompt: &Value, snap: &RuntimeSnapshot) -> Vec<FieldSpec> {
    crate::workflow::form::build_fields(prompt, object_info_of(snap))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn filter_queue_keeps_unmarked_and_own_origin() {
        let queue = json!({
            "queue_running": [
                [1, "a", {}, {"atelyx_origin": "me"}],
                [2, "b", {}, {}],
                [3, "c", {}, {"atelyx_origin": "other"}],
            ],
            "queue_pending": [],
        });
        let kept = filter_queue(&queue, "queue_running", "me");
        assert_eq!(kept.len(), 2, "本机与未标记保留，他机剔除");
    }

    #[test]
    fn images_of_flattens_and_falls_back_title() {
        let entry = json!({
            "prompt": [1, "p1", {"2": {"class_type": "K", "inputs": {"text": "cat"}}}, {"create_time": 100.0}],
            "outputs": {"2": {"images": [{"filename": "x.png", "subfolder": "s", "type": "output"}]}},
            "status": {"status_str": "success"},
        });
        let items = images_of("p1", &entry, "");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].prompt_text, "cat");
        assert_eq!(items[0].created_at, 100.0);
        assert_eq!(items[0].key, "p1:s/x.png");
        assert_eq!(items[0].title, "未命名任务");
    }

    #[test]
    fn random_hex_lengths() {
        assert_eq!(random_hex(16).len(), 32);
        assert_eq!(random_hex(8).len(), 16);
    }

    #[test]
    fn take_fresh_results_dedups_and_skips_other_origin() {
        let mut runtime = Runtime::new();
        runtime.book.origin_id = "me".into();
        let history = json!({
            "p1": {
                "prompt": [1, "p1", {}, {"atelyx_origin": "other"}],
                "outputs": {"2": {"images": [{"filename": "a.png", "type": "output"}]}},
            },
            "p2": {
                "prompt": [2, "p2", {}, {"atelyx_origin": "me"}],
                "outputs": {"2": {"images": [{"filename": "b.png", "type": "output"}]}},
            },
        });
        let fresh = runtime.take_fresh_results(&history);
        assert_eq!(fresh.len(), 1, "他机任务不展示");
        assert_eq!(fresh[0].prompt_id, "p2");
        // 再次取：p1（虽被跳过展示）与 p2 都已登记去重，无新项
        assert!(runtime.take_fresh_results(&history).is_empty());
    }

    fn result(key: &str, local: Option<&str>) -> ResultImage {
        ResultImage { key: key.into(), local: local.map(str::to_string), ..Default::default() }
    }

    #[test]
    fn take_uncopied_and_apply_recapture_roundtrip() {
        let mut runtime = Runtime::new();
        runtime.snap.results = vec![
            result("k1", Some("history/a.png")),
            result("k2", None),
            result("k3", None),
        ];
        // 只取缺副本的，上限生效
        let pending = runtime.take_uncopied(1);
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].key, "k2");

        // 模拟补抓：第一条拿到副本，第二条仍然抓不到
        let mut done = runtime.take_uncopied(10);
        done[0].local = Some("history/retry-k2.png".to_string());
        let filled = runtime.apply_recapture(&done);
        assert_eq!(filled, 1, "只回填成功的那条");
        // 已有副本的 k1 不被覆盖；失败那条累计一轮尝试
        assert_eq!(runtime.snap.results[0].local.as_deref(), Some("history/a.png"));
        assert_eq!(runtime.snap.results[1].local.as_deref(), Some("history/retry-k2.png"));
        assert_eq!(runtime.snap.results[2].copy_attempts, 1);
        assert_eq!(runtime.take_uncopied(10).len(), 1, "只剩 k3");
    }

    #[test]
    fn recapture_gives_up_after_max_attempts() {
        let mut runtime = Runtime::new();
        runtime.snap.results = vec![result("k1", None)];
        for round in 1..=RECAPTURE_ATTEMPTS {
            let pending = runtime.take_uncopied(10);
            assert_eq!(pending.len(), 1, "第 {round} 轮仍应重试");
            runtime.apply_recapture(&pending);
        }
        assert!(runtime.take_uncopied(10).is_empty(), "耗尽轮次后不再占用批次");
    }

    #[test]
    fn merge_results_keeps_one_entry_per_key() {
        let store = CopyStore::new("unused".into());
        let mut runtime = Runtime::new();
        runtime.snap.results = vec![result("k1", Some("history/a.png"))];
        let fresh = vec![
            ResultImage { key: "k1".into(), created_at: 9.0, ..Default::default() },
            ResultImage { key: "k2".into(), created_at: 3.0, ..Default::default() },
        ];
        assert!(runtime.merge_results(fresh, &store));
        assert_eq!(runtime.snap.results.len(), 2, "同 key 只留一条");
        let kept = runtime.snap.results.iter().find(|i| i.key == "k1").unwrap();
        assert_eq!(kept.local.as_deref(), Some("history/a.png"), "保留已带副本路径的那条");
        assert_eq!(runtime.snap.results[0].key, "k2", "按时间倒序");
    }
}
