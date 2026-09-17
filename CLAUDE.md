# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## 项目概览

ByeType 是 Tauri v2 桌面应用（macOS / Windows）：按快捷键说话 → 多模态大模型转写 → 文本优化 → 自动粘贴到光标处；另有截图取字（F6 框选 → 视觉模型提取文字 → 剪贴板 + 预览窗）。所有 AI 行为由可编辑的 Markdown 提示词驱动。托盘常驻、无 Dock 图标。

- 后端 Rust：`src-tauri/src/`，业务逻辑几乎全部在这里
- 前端 React 19 + TypeScript + Vite：`src/`，只做设置 UI 和几个轻量窗口
- 注释与提交信息以中文为主，代码标识符英文

## 🛠️ 开发命令

```bash
npm ci                       # 首次安装依赖
npm run tauri dev            # 本地开发运行（起 Vite :1420 + cargo 编译）
npm run build                # 前端：tsc 类型检查 + vite build，产物 dist/
npx tsc --noEmit             # 仅前端类型检查（仓库没有 eslint / prettier 配置）

cd src-tauri
cargo test                   # Rust 单元测试（全部测试都是 Rust 内联 #[cfg(test)]，前端无测试）
cargo test prompt::tests     # 跑单个模块的测试
cargo test migrates_         # 按名称前缀跑测试
cargo fmt && cargo clippy    # 提 PR 前格式化 + lint
```

注意：`tauri::generate_context!()` 编译期要求 `dist/` 存在，全新 clone 后先跑一次 `npm run build`（或 `mkdir dist`），否则 `cargo test` / `cargo check` 报 frontendDist 不存在。

## 📦 发版规范

- 版本号在三处同步：`package.json`、`src-tauri/Cargo.toml`（连带 Cargo.lock）、`src-tauri/tauri.conf.json`。发版提交格式 `chore: 发布 vX.Y.Z`，随后打 `vX.Y.Z` tag 推送触发 `.github/workflows/release.yml`（macOS arm64 / intel + Windows，产出 updater 用的 `latest.json`）
- 工作流从提交主题自动生成说明，会过滤 chore/docs/refactor/style/test/ci 前缀，所以 feat / fix 的提交主题要写成用户能看懂的中文
- GitHub Release 的版本说明必须使用中文。合并的 PR 标题可能是英文，发版构建完成后必须检查 Release 说明，把英文条目翻译成中文再结束发版（命令：`gh release edit v<版本> --notes "<中文说明>"`）

## 🏗️ 架构

### 多窗口结构

每个窗口是一个独立的 Vite 入口（`vite.config.ts` 的 `rollupOptions.input`），前端之间不共享状态，都通过 Rust 中转：

| HTML 入口 | 窗口 label | 前端 | 用途 |
|---|---|---|---|
| `index.html` | `settings` | React（`src/views/settings/`） | 设置页，按 tab 拆分在 `tabs/` |
| `bubble.html` | `bubble-1..3` | 原生 TS | 光标旁状态气泡，启动时预创建 3 个（`bubble.rs`） |
| `preview.html` | `preview-N` | React | 截图取字结果预览，可钉住（`preview.rs`） |
| `learning.html` | `learning` | React | 自动学习结果审阅（`learning.rs`） |
| `meeting.html` | `meeting` | React（`src/views/meeting/`） | 会议记录：左侧列表，右侧纪要 / 实时转写（`meeting/window.rs`） |
| `screenshot.html` | `screenshot` | 原生 TS | 全屏截图框选遮罩 |

- `bubble-meeting` 是会议记录专用气泡（`bubble::show_meeting`），独立代次，不占听写的 3 个槽位；气泡上的 ✕ 对会议状态触发 `meeting_stop`
- `settings`、`learning`、`meeting` 窗口关闭时只隐藏不销毁（`lib.rs` 的 `on_window_event`）
- 新增窗口要同时：加 HTML + `vite.config.ts` 入口 + `src-tauri/capabilities/default.json` 的 `windows` 列表 + Rust 侧创建逻辑
- Rust → 前端全部走 `app.emit` / `emit_to` 事件；前端所有 `invoke` 封装在 `src/lib/tauri-api.ts`，不要在组件里直接 `invoke`

### 语音主链路

```
shortcut.rs  全局快捷键（2 个语音 + 2 个图像，各绑一个 template_id；toggle / PTT 在按键事件时读配置决定）
  → audio/recorder.rs  cpal 采集 → 混单声道 16kHz → FLAC → base64
  → task/mod.rs  TaskManager 分配 task_id（≤ max_parallel）、CancellationToken、气泡状态
      execute_pipeline:
        ai::transcribe  （with_retry + transcribe_timeout）
        ai::optimize    （template_id 为空或模板提示词为空时跳过，直接输出转写文本）
  → clipboard.rs  快照剪贴板 → 写入 → 模拟 Cmd+V → 可选还原
  → task/history.rs（最近 3 条 + 音频）、usage.rs / timing.rs（JSONL 追加）、learning 记录最终文本
```

- 截图链路平行：`start_extraction` → `capture_screenshot`（macOS 走 `screencapture`，Windows 走 xcap + `screenshot_win32.rs` 遮罩）→ 前端框选回传 crop → `ai::extract_text` → 剪贴板 + `preview::show`
- `local_api.rs`：axum 监听 `127.0.0.1`，`POST /transcribe` 复用 `run_silent_pipeline`（不显示气泡、不动剪贴板、不写历史）。音频输入由 `audio/input.rs` 用 symphonia 解码后统一转 FLAC
- 任务完成/失败的收尾都走 `finish_pipeline` / `finish_extract_pipeline`，靠 `cancel_tokens.remove` 做原子守卫，避免取消与完成竞态

### AI 层（`src-tauri/src/ai/`）

- `mod.rs` 是唯一分发点，按 `ResolvedModel.protocol` 路由：`gemini` → `gemini.rs`；`qwen-omni` → `openai_compat::qwen_omni_*`；`mimo` → `mimo.rs`；`bedrock` → `bedrock.rs`（Converse API，只做文本 / 图像）；`aws-transcribe` → `transcribe_aws.rs`（流式 ASR，只做音频）；其余 → `openai_compat.rs`。DeepSeek 官方 API 按 `base_url` 含 `api.deepseek.com` 单独识别（`is_deepseek`）走 `deepseek.rs`，OpenRouter 上的 deepseek 仍走普通 openai-compat
- 分发集中在三处：`transcribe_with_prompt`（音频）、`extract_text`（图像）、`run_text`（文本，被 `optimize` / `complete_text` / `analyze_correction` 共用），成功后 `record_usage`。**新增 protocol 时三处都要加分支**
- `models.rs` 的 `BUILTIN_MODELS`（id 以 `builtin-` 开头，OpenRouter 的以 `builtin-or-` 开头，AWS 的以 `builtin-bedrock-*` / `builtin-aws-transcribe`）与 `src/core/models.ts` 的 `BUILTIN_MODELS` 是**两份手动镜像**，必须同步改；下线旧 id 时在 `config/migration.rs` 加迁移并补测试
- `resolve_model` 按 protocol 从 `config.models.builtin_api_keys` 取 key。新增服务商 = 新增 `BuiltinApiKeys` 字段（Rust `config/types.rs` + TS `core/types.ts`）+ 两侧 `BUILTIN_MODELS` + `ModelsTab.tsx` 的 key 输入 + `core/types.ts` 的 protocol 联合类型
- **AWS 两个协议不用 API Key**：凭证走 `aws.rs` 的 SDK 默认凭证链（`~/.aws/config` 的 profile，ADA `credential_process` / 静态 key / SSO 都行），配置只有 `models.aws` 里的 profile + region（Bedrock 与 Transcribe 分开，因为同一角色可能只授权其一）。SDK 错误统一经 `aws::map_sdk_error` 转成 `"<Label> API error (<status>): ..."` 以配合 `retry.rs`；SDK 不走 app 的 HTTP 代理设置
- Amazon Transcribe 吃不到提示词，所以 `prompt.rs` 的 `build_optimize_prompt` 在转写模型是它时强制带上 rules / vocabulary / voice-learning（`models::transcribe_needs_post_correction`）
- 真机联调测试带 `#[ignore]`：`cargo test --lib live_ -- --ignored --nocapture`（需要本机 AWS 凭证与 `/tmp/byetype-test-zh.wav` 样本，生成方法见 `transcribe_aws.rs` 测试注释）
- `transport.rs` 是 OpenAI 兼容请求的公共通道；`retry.rs` 的 `with_retry` 靠解析错误字串里的 `(<status>)` 判断 4xx 不重试（408 / 429 除外），所以各 provider 的错误必须保持 `"<Provider> API error (<status>): <body>"` 格式
- `prompt.rs` 把提示词拼成 `<document name="...">` 块：转写 = agent + vocabulary + rules + voice-learning；优化默认只带模板本身，开了 `reuse_transcribe_references` 才再带一遍规则；图像 = 模板本身。内置模板 id → 文件名映射在 `builtin_prompt_filename`

### 配置

- `config/types.rs` 的 `AppConfig` 与 `src/core/types.ts` 的 `AppConfig` 一一对应（serde camelCase），改字段两边一起改
- 新字段必须 `#[serde(default)]` 保证旧 config.json 能加载；字段改名 / 模型 id 下线放 `config/migration.rs`，那里有成套测试
- `config.json` 在 `app_data_dir`（macOS：`~/Library/Application Support/com.byetype.app/`），原子写入且 0600。同目录还有 `prompts/`（用户副本）、`history/`、`usage.jsonl`、`timing.jsonl`、自动学习文档、`meetings/`（会议记录）、`meetings-notes/`（默认纪要导出目录）
- 前端整份读写：`get_config` / `save_config`。`save_config` 会先 `validate`，再重配本机接口，快捷键相关字段变化时重新注册全部快捷键，失败则整体回滚
- 内置提示词目录：生产读 `resource_dir/prompts`，开发态回退到 `src-tauri/prompts`（`commands::resolve_prompts_dir`）

### 自动学习

`learning.rs`：每次粘贴成功记下最终文本；托盘点「自动学习」时拿当前剪贴板文本与之对比，交给 `voice_learning.model_id` 用 `voice-learning-prompt.md` 分析，产出草稿在 `learning` 窗口审阅后追加进学习文档；学习文档内容作为 `voice-learning` document 注入后续转写提示词。

### 会议记录（`src-tauri/src/meeting/`）

```
detector.rs   每 detect_poll_secs 秒 pgrep -x CptHost（Zoom 只在开会时有这个进程），连续 2 次阳性开始、3 次阴性结束
  → session.rs  MeetingManager：Idle → Recording → Finalizing → Idle；手动开始的会话不受探测影响
      capture/mod.rs   专用线程：MicBackend(cpal) + 系统音频后端 → chunker → FLAC → tokio 队列
      capture/macos_tap.rs  Core Audio 进程 tap（14.2+）：CATapDescription(objc) → dlsym 的 AudioHardwareCreateProcessTap → 私有聚合设备 → IOProc
      chunker.rs       双轨对齐、混音、16 kHz、静音处切段（默认 240 s，硬上限 +90 s）、「我 / 对方」能量提示
      run_transcriber  串行转写每段：Transcribe 开说话人分离；LLM 走 meeting-transcribe.md + 上一段结尾
      finalize         停采集 → 等队列排空 → summary.rs（meeting-summary.md，>120k 字 map-reduce）→ store 落盘 → 导出笔记目录
  → store.rs   <app_data>/meetings/<id>/{meta.json, transcript.jsonl, transcript.md, summary.md, audio/}
```

- 配置在 `config.meeting`；纪要模型默认 `builtin-bedrock-claude-sonnet-5`，转写模型为空时跟随听写的转写模型
- 托盘的「开始/停止会议录制」「自动检测 Zoom 会议」由 `tray::update_meeting_items` 刷新；托盘图标由听写与会议两个状态合成（`tray::set_dictation_recording` / `set_meeting_recording`）
- 系统音频需要 `Info.plist` 的 `NSAudioCaptureUsageDescription` 与 TCC「仅系统音频录制」授权；设置页的「请求权限」调用 `macos_tap::probe` 提前触发弹窗
- Windows 侧只有探测（`CptHost.exe`），系统音频后端未实现，会退化成只录麦克风

## 📝 修改提示词的落点

提示词有两份，改功能时**两份都要改**，只改一份会出现「代码里有、实际用不上」或「本机能用、别人装了没有」。

- **内置模板**：`src-tauri/prompts/*.md`，打包进 app 资源，影响新装用户和「恢复默认」，改动要提交进版本库
- **本机在用**：`~/Library/Application Support/com.byetype.app/prompts/*.md`，config.json 里 `voiceTemplates.templates[].prompt` 和 `transcribe.prompts` 指向它，app 实际读的是这份，不进版本库

两份内容可能已经被用户手工改过而不一致，改之前先分别读一遍，按各自的原文风格改，不要用模板整份覆盖本机那份。

改完提交时只提交 `src-tauri/prompts/` 下的改动。
