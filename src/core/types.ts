export type ThemeMode = 'light' | 'dark' | 'system'
/** 界面语言：跟随系统 / 中文 / 英文 */
export type LanguageSetting = 'system' | 'zh-CN' | 'en'

export interface AudioDevice {
  name: string
  isDefault: boolean
}

export interface GeneralConfig {
  shortcut: string
  shortcut2: string
  launchAtLogin: boolean
  theme: ThemeMode
  language: LanguageSetting
  maxRecordingSeconds: number
  microphone: string
  extractShortcut: string
  extractShortcut2: string
  shortcutTemplate: string
  shortcut2Template: string
  extractShortcutTemplate: string
  extractShortcut2Template: string
  shortcutLabel?: string
  shortcut2Label?: string
  extractShortcutLabel?: string
  extractShortcut2Label?: string
  pttMode?: boolean
  overwriteClipboard?: boolean
}

export interface LocalApiConfig {
  enabled: boolean
  port: number
}

export interface LocalApiStatus {
  running: boolean
  port: number | null
  error: string | null
}

export interface ThinkingConfig {
  enabled: boolean
  level: 'LOW' | 'MEDIUM' | 'HIGH'
}

/** 用户自建的 Bedrock 模型：只需要模型 / 推理配置 id 与能力标记，凭证走 AWS profile */
export interface CustomModelEntry {
  id: string
  provider: string
  model: string
  protocol: 'bedrock'
  supportsText: boolean
  supportsVision: boolean
}

/**
 * AWS 接入配置。只存 profile 名与 region，凭证由本机 ~/.aws/config 的凭证链提供
 * （ADA credential_process、静态 key、SSO 都行）。Bedrock 与 Transcribe 分开配，
 * 因为同一个角色可能只授权其中一个服务。
 */
export interface AwsConfig {
  bedrockProfile: string
  bedrockRegion: string
  transcribeProfile: string
  transcribeRegion: string
  /** 'auto' = zh-CN + en-US 多语言识别（首选 zh-CN）；否则填单一语言码，如 'zh-CN' / 'en-US' */
  transcribeLanguage: string
}

export interface ModelsConfig {
  custom: CustomModelEntry[]
  aws: AwsConfig
}

export interface TranscribeConfig {
  modelId: string
  thinking: ThinkingConfig
  prompts: { agent: string; rules: string; vocabulary: string }
}

export interface VoiceLearningConfig {
  modelId: string
  thinking: ThinkingConfig
}

export interface TemplateEntry {
  id: string
  name: string
  prompt: string
}

export interface VoiceTemplatesConfig {
  modelId: string
  thinking: ThinkingConfig
  templates: TemplateEntry[]
  /** 优化阶段是否再带一遍转写参考（专有词汇/转写规则/学习结果）做二次纠错。弱模型建议开启，强模型默认关闭 */
  reuseTranscribeReferences?: boolean
}

export interface ExtractConfig {
  modelId?: string
  thinking?: ThinkingConfig
  prompt: string
  templates: TemplateEntry[]
}

export interface AdvancedConfig {
  transcribeTimeout: number
  optimizeTimeout: number
  maxRetries: number
  maxParallel: number
  proxyEnabled: boolean
  proxyUrl: string
}

export interface S3Config {
  endpoint: string
  region: string
  bucket: string
  accessKey: string
  secretKey: string
  prefix: string
}

export interface BackupConfig {
  s3: S3Config
}

export interface BackupEntry {
  key: string
  size: number
  lastModified: string
}

export interface MeetingPromptsConfig {
  /** 自定义提示词路径，空 = 内置模板 */
  transcribe: string
  summary: string
}

/** 会议记录：检测 Zoom 会议 → 采集麦克风 + 系统音频 → 分段转写 → 纪要 */
export interface MeetingConfig {
  enabled: boolean
  autoDetect: boolean
  captureSystemAudio: boolean
  captureMicrophone: boolean
  /** 空 = 跟随「转写设置」的转写模型 */
  transcribeModelId: string
  summaryModelId: string
  summaryThinking: ThinkingConfig
  chunkSeconds: number
  chunkTimeoutSecs: number
  summaryTimeoutSecs: number
  maxMeetingMinutes: number
  detectPollSecs: number
  /** 纪要导出目录，空 = 应用数据目录下的 meetings-notes */
  notesFolder: string
  keepAudio: boolean
  showWindowOnStart: boolean
  openSummaryWhenDone: boolean
  prompts: MeetingPromptsConfig
}

export interface AppConfig {
  general: GeneralConfig
  localApi: LocalApiConfig
  models: ModelsConfig
  transcribe: TranscribeConfig
  voiceLearning: VoiceLearningConfig
  voiceTemplates: VoiceTemplatesConfig
  extract: ExtractConfig
  advanced: AdvancedConfig
  backup: BackupConfig
  meeting: MeetingConfig
}

export type MeetingPhase = 'idle' | 'recording' | 'finalizing'

export interface MeetingStatus {
  phase: MeetingPhase
  meetingId: string | null
  startedAt: string | null
  elapsedSecs: number
  source: 'auto' | 'manual' | null
  chunksDone: number
  chunksPending: number
  microphone: boolean
  systemAudio: boolean
  warnings: string[]
  lastError: string | null
  lastMeetingId: string | null
}

export type MeetingRecordStatus = 'recording' | 'finalizing' | 'done' | 'summary_failed' | 'failed' | 'interrupted'

export interface MeetingMeta {
  id: string
  title: string
  startedAt: string
  endedAt: string | null
  durationSecs: number
  source: string
  status: MeetingRecordStatus
  chunkCount: number
  failedChunks: number
  transcribeModel: string
  summaryModel: string
  notesPath: string | null
  error: string | null
  microphone: boolean
  systemAudio: boolean
}

export interface TranscriptSegment {
  index: number
  startSecs: number
  endSecs: number
  text: string
  failed: boolean
}

export interface MeetingDetail {
  meta: MeetingMeta
  transcript: string
  summary: string | null
  segments: TranscriptSegment[]
}

export interface MeetingSupport {
  systemAudioSupported: boolean
  reason: string | null
  platform: string
}

export type TaskStatus = 'recording' | 'transcribing' | 'optimizing' | 'retrying' | 'extracting' | 'completed' | 'failed' | 'cancelled'

export interface HistoryRecord {
  id: number
  createdAt: string
  audioPath: string | null
  transcribeText: string | null
  optimizeText: string | null
  status: 'completed' | 'failed' | 'cancelled'
  errorMessage?: string
  recordType?: 'voice' | 'extract'
  screenshotPath?: string | null
  extractText?: string | null
}

export interface RetryStatusUpdate {
  recordId: number
  status: 'transcribing' | 'optimizing' | 'retrying' | 'cancelled' | 'completed' | 'failed'
}

export type UsageScene = 'transcribe' | 'extract' | 'optimize' | 'learn' | 'meeting-transcribe' | 'meeting-summary'

export interface UsageRecord {
  /** 毫秒时间戳 */
  ts: number
  scene: UsageScene
  model: string
  provider: string
  inputTokens: number
  outputTokens: number
}

export interface TimingRecord {
  /** 毫秒时间戳 */
  ts: number
  /** 音频转写阶段耗时（含自动重试），毫秒 */
  transcribeMs: number
  /** 文本优化阶段耗时（含自动重试），未启用时为0 */
  optimizeMs: number
  /** 其他零散耗时（网络建连、粘贴等），毫秒 */
  otherMs: number
  /** 从停止录音到粘贴完成的总耗时，毫秒 */
  totalMs: number
  transcribeModel: string
  transcribeProvider: string
  optimizeModel?: string | null
  optimizeProvider?: string | null
}

export interface UpdateInfo {
  version: string
  body: string | null
}

export type UpdatePhase = 'idle' | 'checking' | 'available' | 'downloading' | 'downloaded' | 'error'

export interface UpdateState {
  phase: UpdatePhase
  info: UpdateInfo | null
  progress: number
  error: string | null
  dismissed: boolean
  checkedOnce: boolean
}
