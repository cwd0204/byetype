import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { open as openDialog } from '@tauri-apps/plugin-dialog'
import { readTextFile, writeTextFile } from '@tauri-apps/plugin-fs'
import type {
  AppConfig, AudioDevice, UpdateInfo, BackupEntry, LocalApiStatus,
  MeetingStatus, MeetingMeta, MeetingDetail, MeetingSupport,
} from '../core/types'

// Config commands
export async function getConfig(): Promise<AppConfig> {
  return invoke<AppConfig>('get_config')
}

export async function saveConfig(config: AppConfig): Promise<boolean> {
  return invoke<boolean>('save_config', { config })
}

export async function getLocalApiStatus(): Promise<LocalApiStatus> {
  return invoke<LocalApiStatus>('get_local_api_status')
}

// Prompt commands
export async function copyBuiltinPrompt(filename: string, force: boolean = false): Promise<string> {
  return invoke<string>('copy_builtin_prompt', { filename, force })
}

export async function isBuiltinPromptPath(path: string): Promise<boolean> {
  return invoke<boolean>('is_builtin_prompt_path', { path })
}

export async function createUserPromptFile(filename: string): Promise<string> {
  return invoke<string>('create_user_prompt_file', { filename })
}

export interface VoiceLearningDocument {
  path: string
  content: string
}

export async function getVoiceLearningDocument(): Promise<VoiceLearningDocument> {
  return invoke<VoiceLearningDocument>('get_voice_learning_document')
}

export async function saveVoiceLearningDocument(
  content: string,
  baseContent: string
): Promise<VoiceLearningDocument> {
  return invoke<VoiceLearningDocument>('save_voice_learning_document', { content, baseContent })
}

export async function getVoiceLearningPromptPath(): Promise<string> {
  return invoke<string>('get_voice_learning_prompt_path')
}

// File operations
export async function selectFile(): Promise<string | null> {
  const result = await openDialog({
    filters: [{ name: 'Markdown', extensions: ['md'] }],
    multiple: false,
  })
  return result as string | null
}

export async function readPromptFile(path: string): Promise<string> {
  return readTextFile(path)
}

export async function writePromptFile(path: string, content: string): Promise<void> {
  return writeTextFile(path, content)
}

// Event listeners
export async function onEvent<T>(event: string, callback: (payload: T) => void): Promise<UnlistenFn> {
  return listen<T>(event, (e) => callback(e.payload))
}

// Launch at login
export async function setLaunchAtLogin(enabled: boolean): Promise<void> {
  await invoke('set_launch_at_login', { enabled })
}

export async function getLaunchAtLogin(): Promise<boolean> {
  return invoke<boolean>('get_launch_at_login')
}

// Microphone
export async function listInputDevices(): Promise<AudioDevice[]> {
  return invoke<AudioDevice[]>('list_input_devices')
}

// Update
export async function checkUpdate(): Promise<UpdateInfo | null> {
  return invoke<UpdateInfo | null>('check_update')
}

export async function downloadUpdate(): Promise<void> {
  return invoke<void>('download_update')
}

export async function installAndRestart(): Promise<void> {
  return invoke<void>('install_and_restart')
}

export interface ConnectivityResult {
  success: boolean
  latencyMs: number
  error: string | null
}

export async function testModelConnectivity(modelId: string): Promise<ConnectivityResult> {
  return invoke<ConnectivityResult>('test_model_connectivity', { modelId })
}

// ==================== Backup ====================

export async function testS3Connection(): Promise<void> {
  return invoke<void>('test_s3_connection')
}

export async function backupToS3(): Promise<string> {
  return invoke<string>('backup_to_s3')
}

export async function listS3Backups(): Promise<BackupEntry[]> {
  return invoke<BackupEntry[]>('list_s3_backups')
}

export async function restoreFromS3(objectKey: string): Promise<void> {
  return invoke<void>('restore_from_s3', { objectKey })
}

export async function backupToLocal(): Promise<string> {
  return invoke<string>('backup_to_local')
}

export async function restoreFromLocal(): Promise<void> {
  return invoke<void>('restore_from_local')
}

// ==================== Meeting ====================

export async function getMeetingStatus(): Promise<MeetingStatus> {
  return invoke<MeetingStatus>('meeting_get_status')
}

export async function startMeeting(): Promise<MeetingStatus> {
  return invoke<MeetingStatus>('meeting_start')
}

export async function stopMeeting(): Promise<MeetingStatus> {
  return invoke<MeetingStatus>('meeting_stop')
}

export async function discardMeeting(): Promise<void> {
  return invoke<void>('meeting_discard')
}

export async function listMeetings(): Promise<MeetingMeta[]> {
  return invoke<MeetingMeta[]>('meeting_list')
}

export async function getMeeting(id: string): Promise<MeetingDetail> {
  return invoke<MeetingDetail>('meeting_get', { id })
}

export async function deleteMeeting(id: string): Promise<void> {
  return invoke<void>('meeting_delete', { id })
}

export async function saveMeetingSummary(id: string, content: string): Promise<void> {
  return invoke<void>('meeting_save_summary', { id, content })
}

export async function regenerateMeetingSummary(id: string): Promise<void> {
  return invoke<void>('meeting_regenerate_summary', { id })
}

/// 用保留的分段音频重新转写（转写失败的会议用）
export async function retranscribeMeeting(id: string): Promise<void> {
  return invoke<void>('meeting_retranscribe', { id })
}

export async function pickNotesFolder(): Promise<string | null> {
  return invoke<string | null>('meeting_pick_notes_folder')
}

export async function getNotesFolder(): Promise<string> {
  return invoke<string>('meeting_notes_folder')
}

export async function revealMeeting(id: string): Promise<void> {
  return invoke<void>('meeting_reveal', { id })
}

export async function checkMeetingSupport(): Promise<MeetingSupport> {
  return invoke<MeetingSupport>('meeting_check_support')
}

export async function requestMeetingPermissions(): Promise<void> {
  return invoke<void>('meeting_request_permissions')
}

export async function openMeetingWindow(meetingId?: string): Promise<void> {
  return invoke<void>('meeting_open_window', { meetingId: meetingId ?? null })
}

export async function closeMeetingWindow(): Promise<void> {
  return invoke<void>('meeting_close_window')
}
