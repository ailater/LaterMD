//! AI 流式写作的应用侧接线(P1「AI 流式写作」)。
//!
//! 后台线程由 `latermd-ai` 的 provider 自带(`AiProvider::stream_complete`
//! 返回线程句柄),本模块只持有接收端:每帧 [`AiState::poll`] 非阻塞收空
//! channel,chunk 翻成 [`crate::state::Message`] 走归约 —— 与搜索
//! (`crate::search`)同款「发起 / 接收 / 收尾」三原语,但文本回编辑器必须
//! 经 Message(铁律三:AI 修改在文本/AST 层,后台线程不触碰 UI 状态)。
//!
//! 不引入 tokio:provider 仍用 `std::sync::mpsc`；每个 chunk 入队后通过
//! 应用层通知器唤醒一帧，空闲期间不再持续重绘。
//!
//! **provider 可切换**(docs/ui-polish.md §6):[`AiRuntime`] 在 Mock /
//! OpenAI 兼容 / Anthropic / Ollama 四者中按 [`AiConfig`] 现场装配,
//! 保存配置即时生效、无需重启。

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::thread::JoinHandle;

use latermd_ai::{
    AiProvider, AnthropicProvider, AnthropicSettings, Chunk, MockProvider, OllamaProvider,
    OllamaSettings, OpenAiProvider, OpenAiSettings,
};

use crate::ai_config::{AiConfig, ProviderKind};
use crate::repaint::RepaintNotifier;
use crate::state::Message;

/// AI commit 后台任务回传：请求序号、发起标签，以及建议或错误。
type CommitResult = (u64, u64, Result<String, String>);

/// 配置页被删参数的内部默认(decisions-pending #108)。
///
/// 「配置页参数精简」删掉采样参数/system prompt/超时/流式的 UI 与落盘
/// 字段后,请求装配仍要有值:这里钉住**删除前** `AiConfig::default()`
/// 的取值,默认请求与删 UI 前逐字段一致(不因删 UI 改变默认请求)。
/// 注意 Anthropic/Ollama 的 adapter `Default` 是「不发采样参数」
/// (decisions-pending #92/#93),与删除前 app 装配的现行为不同 ——
/// 所以这两家不能整体 `..Default::default()` 了事,采样三参数必须
/// 显式给值。超时/流式/system prompt 的默认与 adapter `Default` 同源,
/// 不在此重复(装配处走 `..Default::default()`)。
const INTERNAL_TEMPERATURE: f32 = 0.7;
const INTERNAL_TOP_P: f32 = 1.0;
const INTERNAL_MAX_TOKENS: u32 = 2048;

/// 生效的 provider 运行时。
///
/// 枚举而非 trait 对象:只有四种实现,编译期穷尽匹配比动态分发更省事,
/// 也让「Mock 的同步合成」这类独有方法不必塞进公共 trait。
#[derive(Clone)]
pub enum AiRuntime {
    /// 内置演示 provider:不联网。
    Mock(MockProvider),
    /// OpenAI 兼容端点(参数见 [`OpenAiSettings`])。
    OpenAi(OpenAiProvider),
    /// Anthropic messages API(参数见 [`AnthropicSettings`])。
    Anthropic(AnthropicProvider),
    /// Ollama 本地 `/api/chat`(参数见 [`OllamaSettings`],无鉴权)。
    Ollama(OllamaProvider),
}

impl AiRuntime {
    /// 状态栏 / 设置页显示名。
    pub fn label(&self) -> &'static str {
        match self {
            Self::Mock(_) => "Mock",
            Self::OpenAi(_) => "OpenAI 兼容",
            Self::Anthropic(_) => "Anthropic",
            Self::Ollama(_) => "Ollama 本地",
        }
    }

    fn stream_complete(&self, prompt: &str, tx: Sender<Chunk>) -> JoinHandle<()> {
        match self {
            Self::Mock(provider) => provider.stream_complete(prompt, tx),
            Self::OpenAi(provider) => provider.stream_complete(prompt, tx),
            Self::Anthropic(provider) => provider.stream_complete(prompt, tx),
            Self::Ollama(provider) => provider.stream_complete(prompt, tx),
        }
    }

    /// 同步生成 commit subject(commit 建议是「一行结果」,流式对它无意义)。
    ///
    /// Mock 走关键词合成;三家真实 provider 走一次非流式请求并取首行
    /// (模型偶尔会带解释性前缀行,首行即 subject 的约定在 prompt 里已写明)。
    pub fn commit_subject(&self, prompt: &str) -> Result<String, String> {
        match self {
            Self::Mock(provider) => Ok(provider.mock_commit_subject(prompt)),
            Self::OpenAi(provider) => sync_subject(provider.complete_sync(prompt)),
            Self::Anthropic(provider) => sync_subject(provider.complete_sync(prompt)),
            Self::Ollama(provider) => sync_subject(provider.complete_sync(prompt)),
        }
    }
}

/// 非流式补全 → 首行 subject(三家真实 provider 共用一条通路)。
fn sync_subject(result: Result<String, latermd_ai::AiError>) -> Result<String, String> {
    result
        .map(|text| first_line(&text).to_owned())
        .map_err(|error| format!("AI 请求失败:{error}"))
}

/// 正文首行(去空白;空正文给空串)。
fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or("").trim()
}

/// AI 流式状态:运行时 + 当前流的接收端 + 进行中标志。
///
/// `streaming` 是普通 bool 而非 AtomicBool:发起与收尾都只发生在 UI 线程
/// 的归约里(单线程访问),原子性无从谈起;它存在的意义是防重入 —— 流式
/// 进行中再次触发命令在 [`AiState::start_with_repaint`] 入口被忽略。
pub struct AiState {
    /// 生效的 provider;`pub(crate)` 仅为测试注入零间隔 provider。
    pub(crate) runtime: AiRuntime,
    /// 当前流的接收端;`finish` 时 drop,发送端下一次 send 失败静默收尾。
    pub(crate) rx: Option<Receiver<Chunk>>,
    /// 是否有流在途(防重入标志,见类型文档)。
    pub(crate) streaming: bool,
    /// 最近一次真实发起的 prompt 原文(AI 指令卡的状态匹配键):
    /// 指令文本与它相等 → 流式中「进行中」/结束「已完成」,否则「未执行」。
    /// 只增不清是刻意的:`finish` 后保留才能显示「已完成」,失效时机只有
    /// 两个——流失败([`crate::state::Message::AiFailed`] 归约里清)与发起
    /// 标签被关闭(`State::remove_tab` 作废流时清)。流绑定发起标签
    /// (`State::ai_active_tab`),切标签不作废:别的标签里文本不匹配的
    /// 指令卡自然显示「未执行」,切回发起标签仍能正确显示状态。
    pub(crate) last_prompt: Option<String>,
    /// 当前生效的 AI 配置(provider/端点/模型)。与 `runtime` 同源:
    /// 保存配置即重新装配 runtime,两者不会各说各话。
    pub(crate) config: AiConfig,
    /// 最新一次 commit 建议请求序号；后台结果必须匹配才允许落状态。
    commit_seq: u64,
    /// commit 建议后台任务的一次性结果；`Some` 表示任务仍在进行。
    commit_rx: Option<Receiver<CommitResult>>,
}

impl Default for AiState {
    fn default() -> Self {
        Self {
            runtime: AiRuntime::Mock(MockProvider::new()),
            rx: None,
            streaming: false,
            last_prompt: None,
            config: AiConfig::default(),
            commit_seq: 0,
            commit_rx: None,
        }
    }
}

impl AiState {
    /// 按配置 + key 装配运行时。provider 是唯一开关,接口方式随 provider
    /// 派生(`api_style` 已删,decisions-pending #94)。需要 key 的 provider
    /// (OpenAI 兼容/Anthropic,共用同一凭据通道)在 key 缺失时照样装配
    /// —— 请求会被命令闸门拦在前面(`requires_key` 为真而凭据为空时,
    /// 四条 AI 命令入口先落「未配置 key」提示,见 `State::ai_key_gate`);
    /// Ollama 本地无鉴权,无 key 照常工作。调用方负责先 `normalize`
    /// (两个调用点 `load_preferences`/`apply_ai_config` 都已做)。
    ///
    /// 采样参数/system prompt/超时/流式已随「配置页参数精简」从配置删除
    /// (decisions-pending #108):这里按 [`INTERNAL_TEMPERATURE`] 等内部
    /// 默认装配(= 删除前 `AiConfig::default()`),默认请求不因删 UI 改变;
    /// 超时/流式/system prompt 走各 adapter `Default`(与删除前出厂一致)。
    pub fn set_provider(&mut self, config: AiConfig, api_key: Option<&str>) {
        let key = api_key.unwrap_or_default();
        self.runtime = match config.provider {
            ProviderKind::Mock => AiRuntime::Mock(MockProvider::new()),
            ProviderKind::OpenAiCompatible => AiRuntime::OpenAi(OpenAiProvider::with_settings(
                key,
                OpenAiSettings {
                    base_url: config.base_url.clone(),
                    model: config.model.clone(),
                    temperature: Some(INTERNAL_TEMPERATURE),
                    top_p: Some(INTERNAL_TOP_P),
                    max_tokens: Some(INTERNAL_MAX_TOKENS),
                    ..OpenAiSettings::default()
                },
            )),
            ProviderKind::Anthropic => AiRuntime::Anthropic(AnthropicProvider::with_settings(
                key,
                AnthropicSettings {
                    base_url: config.base_url.clone(),
                    model: config.model.clone(),
                    temperature: Some(INTERNAL_TEMPERATURE),
                    top_p: Some(INTERNAL_TOP_P),
                    max_tokens: INTERNAL_MAX_TOKENS,
                    ..AnthropicSettings::default()
                },
            )),
            ProviderKind::Ollama => {
                AiRuntime::Ollama(OllamaProvider::with_settings(OllamaSettings {
                    base_url: config.base_url.clone(),
                    model: config.model.clone(),
                    temperature: Some(INTERNAL_TEMPERATURE),
                    top_p: Some(INTERNAL_TOP_P),
                    num_predict: Some(INTERNAL_MAX_TOKENS as i32),
                    ..OllamaSettings::default()
                }))
            }
        };
        self.config = config;
    }

    /// 当前 provider 是否需要 key 才能发起命令。
    ///
    /// 取自**配置**而非运行时:网关判定与「实际用哪个 provider 发请求」
    /// 是两件事 —— 配置说要 key 就是「这条命令必须带 key」,即使测试把
    /// 运行时换成 Mock 也仍按配置拦截(不发出真实请求就能测闸门)。
    pub fn requires_key(&self) -> bool {
        self.config.provider.requires_key()
    }

    /// 状态栏显示名(Mock / OpenAI 兼容 / Anthropic / Ollama 本地)。
    pub fn provider_label(&self) -> &'static str {
        self.runtime.label()
    }

    /// 发起一次流式续写。已在流式中则忽略(防重入),返回是否真的发起了。
    ///
    /// provider 在自己的后台线程里按节奏发块,本方法同步返回；默认通知器
    /// 只供无头测试/兼容调用，生产入口使用 [`Self::start_with_repaint`]。
    #[cfg(test)]
    pub fn start(&mut self, prompt: &str) -> bool {
        self.start_with_repaint(prompt, RepaintNotifier::default())
    }

    /// 发起一次会主动唤醒 UI 的流式续写。输入 prompt 来自命令归约，通知器
    /// 来自应用状态；返回值供归约判断是否锁定发起标签。桥接线程只转发
    /// chunk 并请求重绘，不修改文档，也不产生写盘或网络之外的副作用。
    pub fn start_with_repaint(&mut self, prompt: &str, repaint: RepaintNotifier) -> bool {
        if self.streaming {
            return false;
        }
        let Ok((tx, rx)) = repaint.notifying_channel("latermd-ai-repaint") else {
            return false;
        };
        let worker: JoinHandle<()> = self.runtime.stream_complete(prompt, tx);
        // 不 join:接收端在 `finish` 时 drop,worker 下一次 send 失败即退出
        // (latermd-ai 的取消约定);与搜索服务同款,不滞留句柄。
        drop(worker);
        self.rx = Some(rx);
        self.streaming = true;
        self.last_prompt = Some(prompt.to_owned());
        true
    }

    /// 发起 commit message 后台任务。目录来自当前文档或文件树根，预算来自
    /// 当前 AI 配置；worker 完成“采 diff → 截取 prompt → HTTP/Mock 补全”
    /// 后只回传结果并唤醒 UI，不接触标签、提示行或磁盘写入状态。
    pub fn start_commit(
        &mut self,
        dir: PathBuf,
        tab_id: u64,
        context_budget: Option<usize>,
        repaint: RepaintNotifier,
    ) -> Result<bool, String> {
        if self.commit_rx.is_some() {
            return Ok(false);
        }
        self.commit_seq = self.commit_seq.wrapping_add(1);
        let seq = self.commit_seq;
        let runtime = self.runtime.clone();
        let (tx, rx) = repaint
            .notifying_channel("latermd-ai-commit-repaint")
            .map_err(|error| format!("AI commit 通知线程启动失败:{error}"))?;
        self.commit_rx = Some(rx);
        let spawned = std::thread::Builder::new()
            .name("latermd-ai-commit".to_owned())
            .spawn(move || {
                // Git 读取和真实 provider 的阻塞 HTTP 都留在工作线程；结果
                // 回到 reducer 后才允许显示建议或写发起标签的提示行。
                let result = crate::git_diff::uncommitted_diff(&dir).and_then(|diff| {
                    if diff.trim().is_empty() {
                        return Err("没有未提交的改动,无需生成 commit message".to_owned());
                    }
                    let prompt =
                        latermd_ai::commit_message_prompt_with_budget(&diff, context_budget);
                    runtime.commit_subject(&prompt)
                });
                let _ = tx.send((seq, tab_id, result));
            });
        if let Err(error) = spawned {
            self.commit_rx = None;
            return Err(format!("AI commit 后台线程启动失败:{error}"));
        }
        Ok(true)
    }

    /// 非阻塞收空 channel,chunk 翻成消息。成功结束发 [`Message::AiDone`];
    /// 失败块(provider 契约:`done` 且 `delta` 非空)发 [`Message::AiFailed`]。
    /// `streaming` 标志与接收端不清在这里:生命周期收口在归约侧的
    /// `finish`(状态变更只发生在 `apply`)。
    ///
    /// channel 断开却没等到收尾块(worker 异常退出)也按失败收尾:流式
    /// 标志绝不能卡死,否则后续触发会被防重入永远拦下。
    pub fn poll(&mut self) -> Vec<Message> {
        let mut messages = Vec::new();
        if let Some(rx) = self.rx.as_ref() {
            loop {
                match rx.try_recv() {
                    Ok(chunk) if !chunk.done => {
                        messages.push(Message::AiChunk { delta: chunk.delta })
                    }
                    Ok(chunk) => {
                        if chunk.delta.is_empty() {
                            messages.push(Message::AiDone);
                        } else {
                            messages.push(Message::AiFailed(chunk.delta));
                        }
                        break; // 结束块是流的最后一条
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        let terminated = messages
                            .iter()
                            .any(|m| matches!(m, Message::AiDone | Message::AiFailed(_)));
                        if !terminated {
                            messages.push(Message::AiFailed("AI 流意外中断".into()));
                        }
                        break;
                    }
                }
            }
        }
        if let Some(rx) = self.commit_rx.as_ref() {
            match rx.try_recv() {
                Ok((seq, tab_id, result)) => messages.push(Message::AiCommitFinished {
                    seq,
                    tab_id,
                    result,
                }),
                Err(TryRecvError::Disconnected) => {
                    messages.push(Message::AiCommitFinished {
                        seq: self.commit_seq,
                        // 线程异常退出时拿不到原标签；0 不会命中真实 tab，
                        // reducer 只收口任务，避免把错误写到当前其他文档。
                        tab_id: 0,
                        result: Err("AI commit 后台任务意外中断".to_owned()),
                    });
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        messages
    }

    /// 收尾(归约侧 [`Message::AiDone`] / [`Message::AiFailed`] 调用):
    /// 清进行中标志并丢弃接收端(及其内积压事件),允许下一次发起。
    pub fn finish(&mut self) {
        self.rx = None;
        self.streaming = false;
    }

    /// 是否有流在途，供防重入与界面状态判断；重绘由 chunk 入队通知驱动。
    pub fn is_streaming(&self) -> bool {
        self.streaming
    }

    /// commit 建议是否仍在后台生成；命令重复触发据此防重入。
    pub fn is_commit_running(&self) -> bool {
        self.commit_rx.is_some()
    }

    /// 收口 commit 后台任务。只有最新请求序号匹配时才清接收端并返回 true；
    /// 迟到结果返回 false，调用方不得改建议浮窗或任何标签提示。
    pub fn finish_commit(&mut self, seq: u64) -> bool {
        if seq != self.commit_seq || self.commit_rx.is_none() {
            return false;
        }
        self.commit_rx = None;
        true
    }

    /// 清掉「最近一次发起的 prompt」(指令卡状态随之回到未执行)。只在两个
    /// 失效时机调用:流失败归约、换文档(`State::load_document`);`finish`
    /// 本身不清,否则流刚结束「已完成」就不可见。
    pub(crate) fn forget_last_prompt(&mut self) {
        self.last_prompt = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// 把消息序列里的正文块拼起来,便于断言。
    fn deltas(messages: &[Message]) -> Vec<String> {
        messages
            .iter()
            .filter_map(|m| match m {
                Message::AiChunk { delta } => Some(delta.clone()),
                _ => None,
            })
            .collect()
    }

    /// 完整生命周期:start 后 poll 收到正文块,自然结束收到 AiDone;
    /// 正文块拼接无损、非空,且没有失败块。用零间隔 provider 保持测试快速
    /// (默认 100ms/块 × 30-50 块是联调节奏,不是测试节奏)。
    #[test]
    fn start_poll_yields_chunks_then_done() {
        let mut ai = AiState {
            runtime: AiRuntime::Mock(MockProvider::with_interval(Duration::ZERO)),
            ..AiState::default()
        };
        assert!(ai.start("帮我续写这段设计文档"));
        assert!(ai.is_streaming());

        let mut messages = Vec::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            messages.extend(ai.poll());
            // 收到结束块即停(is_streaming 要等归约侧 finish 才清,不能当循环条件)
            if messages
                .iter()
                .any(|m| matches!(m, Message::AiDone | Message::AiFailed(_)))
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }

        assert!(
            messages.iter().any(|m| matches!(m, Message::AiDone)),
            "自然结束发 AiDone"
        );
        assert!(!deltas(&messages).concat().is_empty(), "正文块非空");
        assert!(
            messages.iter().all(|m| !matches!(m, Message::AiFailed(_))),
            "成功流无失败块"
        );
    }

    /// 防重入:流式进行中再次 start 被忽略(返回 false,不产生第二个流)。
    #[test]
    fn start_while_streaming_is_ignored() {
        let mut ai = AiState::default();
        assert!(ai.start("随便写点什么"));
        assert!(!ai.start("第二次触发"), "流式中再触发被忽略");
        // 收尾后可再次发起
        ai.finish();
        assert!(ai.start("收尾后重新发起"));
    }

    /// 失败契约:done 且 delta 非空的块翻成 AiFailed(人工注入 channel,
    /// 不经 provider —— Mock 不产生失败块,OpenAI adapter 的失败路径在此收口)。
    #[test]
    fn failure_chunk_maps_to_ai_failed() {
        let mut ai = AiState {
            runtime: AiRuntime::Mock(MockProvider::new()),
            rx: None,
            streaming: true,
            last_prompt: None,
            config: AiConfig::default(),
            commit_seq: 0,
            commit_rx: None,
        };
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(Chunk {
            delta: "额度用尽".into(),
            done: true,
        })
        .unwrap();
        ai.rx = Some(rx);

        assert_eq!(ai.poll(), vec![Message::AiFailed("额度用尽".into())]);
    }

    /// 断连兜底:发送端已撤而收尾块未到(worker 异常退出)→ 补发 AiFailed,
    /// 流式标志不卡死;正常收到过收尾块则不重复报错。
    #[test]
    fn disconnected_channel_without_done_becomes_ai_failed() {
        let mut ai = AiState {
            runtime: AiRuntime::Mock(MockProvider::new()),
            rx: None,
            streaming: true,
            last_prompt: None,
            config: AiConfig::default(),
            commit_seq: 0,
            commit_rx: None,
        };
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(Chunk {
            delta: "半截".into(),
            done: false,
        })
        .unwrap();
        drop(tx);
        ai.rx = Some(rx);
        assert_eq!(
            ai.poll(),
            vec![
                Message::AiChunk {
                    delta: "半截".into()
                },
                Message::AiFailed("AI 流意外中断".into()),
            ]
        );

        // 已有收尾块在前:断连不再追加第二条失败
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(Chunk {
            delta: String::new(),
            done: true,
        })
        .unwrap();
        drop(tx);
        ai.rx = Some(rx);
        assert_eq!(ai.poll(), vec![Message::AiDone]);
    }

    /// poll 不消费结束块之后的生命周期:标志保持到归约侧 finish;空闲 poll
    /// (无 channel)返回空。
    #[test]
    fn poll_without_channel_is_empty_and_finish_clears_state() {
        let mut ai = AiState::default();
        assert!(ai.poll().is_empty());
        ai.start("x");
        ai.finish();
        assert!(!ai.is_streaming());
        assert!(ai.poll().is_empty(), "finish 丢弃接收端,不再产出消息");
    }

    /// 运行时切换:Mock 不需要 key;OpenAI 兼容需要 key,且配置被记进
    /// `config`(闸门与状态栏显示名随之变化)。
    #[test]
    fn set_provider_switches_runtime_and_key_requirement() {
        let mut ai = AiState::default();
        assert!(!ai.requires_key());
        assert_eq!(ai.provider_label(), "Mock");

        let config = AiConfig {
            provider: ProviderKind::OpenAiCompatible,
            model: "deepseek-chat".to_owned(),
            base_url: "https://api.deepseek.com/v1".to_owned(),
            context_kb: 0,
        };
        ai.set_provider(config.clone(), Some("placeholder-key"));
        assert!(ai.requires_key());
        assert_eq!(ai.provider_label(), "OpenAI 兼容");
        assert_eq!(ai.config, config, "配置跟着运行时一起换");

        // 切回 Mock:闸门随之打开
        ai.set_provider(AiConfig::default(), None);
        assert!(!ai.requires_key());
    }

    /// 四 provider 默认装配回归(#58 配置页参数精简):配置里已无采样
    /// 参数/system prompt/超时/流式,`set_provider` 用内部默认装配 ——
    /// 逐字段断言与**删除前** `AiConfig::default()` 装配出的 settings
    /// 一致,即默认请求体组装输入不因删 UI 改变(请求体本身由
    /// latermd-ai 各 adapter 的 `request_body_*` 测试钉住)。顺带断言
    /// 端点/模型照常透传、Ollama 无 key 也装配。
    #[test]
    fn set_provider_dispatches_all_kinds_with_pre_removal_defaults() {
        let mut ai = AiState::default();

        // Mock:不联网不读参数,装配出 Mock 运行时
        ai.set_provider(AiConfig::default(), None);
        assert!(!ai.requires_key());
        assert_eq!(ai.provider_label(), "Mock");
        assert!(matches!(ai.runtime, AiRuntime::Mock(_)));

        // OpenAI 兼容:0.7/1.0/2048/流式/60s(= adapter 出厂,也是删前默认)
        ai.set_provider(
            AiConfig {
                provider: ProviderKind::OpenAiCompatible,
                base_url: "https://api.deepseek.com/v1".to_owned(),
                model: "deepseek-chat".to_owned(),
                context_kb: 0,
            },
            Some("placeholder-key"),
        );
        assert!(ai.requires_key());
        assert_eq!(ai.provider_label(), "OpenAI 兼容");
        let AiRuntime::OpenAi(provider) = &ai.runtime else {
            panic!("应装配 OpenAI 兼容运行时");
        };
        let settings = provider.settings();
        assert_eq!(settings.base_url, "https://api.deepseek.com/v1");
        assert_eq!(settings.model, "deepseek-chat");
        assert_eq!(settings.temperature, Some(INTERNAL_TEMPERATURE));
        assert_eq!(settings.top_p, Some(INTERNAL_TOP_P));
        assert_eq!(settings.max_tokens, Some(INTERNAL_MAX_TOKENS));
        assert_eq!(settings.system_prompt, "");
        assert!(settings.stream);
        assert_eq!(settings.timeout_secs, 60);

        // Anthropic:删前 app 装配是塞 0.7/1.0(不是 adapter 出厂的 None,
        // 见 INTERNAL_* 常量注释);max_tokens 是协议必填,恒 2048
        ai.set_provider(
            AiConfig {
                provider: ProviderKind::Anthropic,
                base_url: "https://api.anthropic.com".to_owned(),
                model: "claude-sonnet-4-5".to_owned(),
                context_kb: 0,
            },
            Some("placeholder-key"),
        );
        assert!(ai.requires_key());
        assert_eq!(ai.provider_label(), "Anthropic");
        let AiRuntime::Anthropic(provider) = &ai.runtime else {
            panic!("应装配 Anthropic 运行时");
        };
        let settings = provider.settings();
        assert_eq!(settings.base_url, "https://api.anthropic.com");
        assert_eq!(settings.temperature, Some(INTERNAL_TEMPERATURE));
        assert_eq!(settings.top_p, Some(INTERNAL_TOP_P));
        assert_eq!(settings.max_tokens, INTERNAL_MAX_TOKENS);
        assert_eq!(settings.system_prompt, "");
        assert!(settings.stream);
        assert_eq!(settings.timeout_secs, 60);

        // Ollama:删前默认是 Some(0.7)/Some(1.0)/Some(2048),超时本地推理档 120
        ai.set_provider(
            AiConfig {
                provider: ProviderKind::Ollama,
                base_url: "http://127.0.0.1:11434".to_owned(),
                model: "llama3.1".to_owned(),
                context_kb: 0,
            },
            None,
        );
        assert!(!ai.requires_key(), "Ollama 无 key 照常工作");
        assert_eq!(ai.provider_label(), "Ollama 本地");
        let AiRuntime::Ollama(provider) = &ai.runtime else {
            panic!("应装配 Ollama 运行时");
        };
        let settings = provider.settings();
        assert_eq!(settings.base_url, "http://127.0.0.1:11434");
        assert_eq!(settings.temperature, Some(INTERNAL_TEMPERATURE));
        assert_eq!(settings.top_p, Some(INTERNAL_TOP_P));
        assert_eq!(
            settings.num_predict,
            Some(INTERNAL_MAX_TOKENS as i32),
            "max_tokens 映射为 num_predict"
        );
        assert_eq!(settings.system_prompt, "");
        assert!(settings.stream);
        assert_eq!(settings.timeout_secs, 120);
    }

    /// 首行提取:commit subject 只取第一行(模型常带解释性后续行)。
    #[test]
    fn first_line_takes_only_the_subject() {
        assert_eq!(
            first_line("docs: 新增 README\n\n说明……"),
            "docs: 新增 README"
        );
        assert_eq!(first_line(""), "");
        assert_eq!(first_line("  \n x"), "");
    }
}
