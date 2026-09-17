import type { MessageBundle } from '../types'

// 区域：about。key 统一用 'about.xxx' 前缀；zh 与 en 的 key 必须一一对应。
export const about: MessageBundle = {
  zh: {
    'about.title': '关于',
    'about.version': '版本 {version}',
    'about.checking': '检查中...',
    'about.checkUpdate': '检查更新',
    'about.upToDate': '已是最新版本',
    'about.versionAvailable': 'v{version} 可用',
    'about.updateNow': '立即更新',
    'about.newVersion': '新版本',
    'about.later': '稍后',
    'about.downloading': '正在下载 v{version}',
    'about.ready': 'v{version} 已准备就绪',
    'about.installRestart': '重启并安装',
    'about.currentVersion': '当前版本',
  },
  en: {
    'about.title': 'About',
    'about.version': 'Version {version}',
    'about.checking': 'Checking...',
    'about.checkUpdate': 'Check for updates',
    'about.upToDate': 'You are up to date',
    'about.versionAvailable': 'v{version} available',
    'about.updateNow': 'Update now',
    'about.newVersion': 'New',
    'about.later': 'Later',
    'about.downloading': 'Downloading v{version}',
    'about.ready': 'v{version} is ready',
    'about.installRestart': 'Restart & install',
    'about.currentVersion': 'Current version',
  },
}
