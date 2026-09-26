/**
 * 文案表（v1 只出简体中文，键为语义名）。
 * 规则：错误一律按后端给的 messageKey 查本表，查不到就显示通用兜底文案 ——
 * UI 永远不显示原始错误码、内部状态名或 HTTP 状态。
 */

export type MessageKey = string;

const MESSAGES: Record<MessageKey, string> = {
  'app.name': 'Notera',
  'app.tagline': '本地优先 · 自有服务器同步',

  /* 侧栏 */
  'sidebar.allNotes': '全部笔记',
  'sidebar.folders': '文件夹',
  'sidebar.trash': '最近删除',
  'sidebar.newFolder': '新建文件夹',
  'sidebar.rename': '重命名',
  'sidebar.moveTo': '移动到…',
  'sidebar.moveHere': '移动到这里',
  'sidebar.deleteFolder': '删除文件夹',
  'sidebar.deleteFolderHint': '删除文件夹不会删除其中的笔记，它们会回到"未归类"。',
  'sidebar.root': '未归类',
  'sidebar.collapse': '折叠侧栏',
  'sidebar.expand': '展开侧栏',
  'sidebar.newSubfolder': '在这里新建子文件夹',

  /* 列表 / 搜索 */
  'list.searchPlaceholder': '搜索笔记',
  'list.searching': '搜索中…',
  'list.noResults': '没有找到相关内容',
  'list.noResultsHint': '换个词试试，中文两个字也能搜。',
  'list.empty': '这里还没有笔记',
  'list.emptyHint': '按 Ctrl+N 或在右侧开始写。',
  'list.emptyFolder': '这个文件夹是空的',
  'list.newNote': '新建笔记',
  'list.pin': '固定',
  'list.unpin': '取消固定',
  'list.hasAttachment': '含附件',
  'list.trashMode': '最近删除',
  'list.restore': '恢复',
  'list.purge': '彻底删除',
  'list.purgeConfirm': '彻底删除后无法找回（会同时从服务器上抹掉这条记录）。确认删除？',
  'list.purgeConfirmShort': '确认彻底删除？',
  'list.cancel': '取消',
  'list.confirm': '确认',
  'list.loadMore': '加载更多',
  'list.rowsLoaded': '已载入 {count} 条',

  /* 编辑器 */
  'editor.untitled': '无标题',
  'editor.placeholder': '开始写…',
  'editor.saved': '已保存',
  'editor.saving': '保存中',
  'editor.unsaved': '未保存',
  'editor.readOnly': '只读',
  'editor.staleTitle': '这条笔记在别处被改动了',
  'editor.staleBody': '为避免覆盖，已显示对方版本。你的改动仍保留在下方，可自行取舍。',
  'editor.showMyDraft': '查看我的版本',
  'editor.useMyDraft': '用我的版本继续编辑',
  'editor.discardMyDraft': '放弃我的改动',
  'editor.versionTooNew': '这条笔记由更新版本的 Notera 保存，请升级后编辑（当前可查看）。',
  'editor.unknownBlock': '暂不支持的内容（已原样保留）',
  'editor.attachmentMissing': '附件不在这台设备上，正在等待下载',
  'editor.attachmentDownload': '获取附件',
  'editor.imageMissing': '图片不在这台设备上',
  'editor.blockCode': '代码',
  'editor.blockQuote': '引用',
  'editor.blockHeading': '标题 {level}',
  'editor.blockParagraph': '正文',
  'editor.blockListOrdered': '编号列表',
  'editor.blockListBullet': '项目列表',
  'editor.blockChecklist': '清单',
  'editor.blockRule': '分隔线',
  'editor.blockImage': '图片',
  'editor.blockAttachment': '附件',
  'editor.deleteBlock': '删除这一块',

  /* 工具条 */
  'tb.bold': '加粗',
  'tb.italic': '斜体',
  'tb.underline': '下划线',
  'tb.strike': '删除线',
  'tb.code': '行内代码',
  'tb.highlight': '高亮',
  'tb.link': '链接',
  'tb.linkPrompt': '输入链接地址',
  'tb.linkApply': '应用',
  'tb.type': '段落样式',
  'tb.checklist': '清单',
  'tb.indent': '增加缩进',
  'tb.outdent': '减少缩进',
  'tb.rule': '分隔线',
  'tb.attach': '插入附件',
  'tb.undo': '撤销',
  'tb.redo': '重做',

  /* 同步徽标（用户只看四态） */
  'sync.synced': '已同步',
  'sync.syncing': '正在同步',
  'sync.offline': '离线',
  'sync.failed': '同步失败',
  'sync.retry': '重试',
  'sync.detail': '同步状态',
  'sync.syncNow': '立即同步',
  'sync.progress': '{done}/{total}',
  'sync.lastRound': '上一次：{when}',
  'sync.never': '尚未同步',
  'sync.localReady': '本地已保存，网络恢复后会自动继续。',
  'sync.plainHttp': '当前使用未加密传输，仅建议在内网。',
  'sync.insecureWarn': '未加密传输：只有内网才建议这样设置。',

  /* 冲突收件箱 */
  'conflict.title': '需要处理的版本',
  'conflict.empty': '没有需要处理的内容',
  'conflict.emptyHint': '同步出现分歧时，这里会列出待你决定的条目。',
  'conflict.keepBoth': '保留两份',
  'conflict.replaceWithThis': '用这个替换',
  'conflict.mine': '我这台设备',
  'conflict.theirs': '另一处改动',
  'conflict.manualMerge': '我来合并',
  'conflict.mergeHint': '复制任一份内容，粘贴整理成你想要的样子后点"用这个替换"。',
  'conflict.openNote': '打开笔记',
  'conflict.count': '{count} 条待处理',

  /* 设置 */
  'settings.title': '设置',
  'settings.account': '账户',
  'settings.accountBaseUrl': '服务器地址',
  'settings.accountUser': '用户名',
  'settings.accountPassword': '口令',
  'settings.accountPasswordSet': '已保存口令（留空则不修改）',
  'settings.accountRootPrefix': '存储前缀',
  'settings.tls': '证书校验',
  'settings.tlsStrict': '严格（推荐）',
  'settings.tlsPin': '固定证书指纹',
  'settings.tlsCaBundle': '使用自定根证书',
  'settings.tlsInsecure': '不校验（仅限内网 HTTP）',
  'settings.proxy': '代理',
  'settings.proxyMode': '代理模式',
  'settings.proxyDirect': '不使用代理',
  'settings.proxySystem': '跟随系统',
  'settings.proxyHttp': 'HTTP',
  'settings.proxyHttps': 'HTTPS',
  'settings.proxySocks5': 'SOCKS5',
  'settings.proxyHost': '地址',
  'settings.proxyPort': '端口',
  'settings.proxyUser': '用户名',
  'settings.proxyPassword': '口令',
  'settings.proxyBypass': '绕过（每行一个主机/网段/*.域名）',
  'settings.save': '保存账户',
  'settings.saved': '已保存',
  'settings.cleared': '已停用',
  'settings.enabled': '启用同步',
  'settings.theme': '外观',
  'settings.themeSystem': '跟随系统',
  'settings.themeLight': '浅色',
  'settings.themeDark': '深色',
  'settings.fontScale': '正文字号',
  'settings.data': '数据',
  'settings.export': '导出全部数据',
  'settings.import': '导入数据',
  'settings.importModeEmpty': '只在空库时导入',
  'settings.importModeMerge': '合并进现有库',
  'settings.backup': '备份本地库',
  'settings.restore': '从备份恢复',
  'settings.report': '结果：{text}',
  'settings.storage': '本地占用 {size}',
  'settings.notesCount': '{count} 条笔记 · {folders} 个文件夹',
  'settings.trashCount': '最近删除 {count} 条',
  'settings.attachments': '{count} 个附件',
  'settings.inflight': '待处理任务 {count}',
  'settings.shortcuts': '键盘快捷键',
  'settings.trayHint': '关闭时留在系统托盘',
  'settings.trayUnavailable': '这台设备不支持常驻托盘',
  'settings.transparency': '窗口透明效果',
  'settings.path': '运行方式',
  'settings.pathTauri': '桌面壳',
  'settings.pathHttp': '本地服务 + 浏览器',
  'settings.close': '关闭',

  /* 状态与异常态 */
  'state.loading': '正在读取本地库…',
  'state.boot': '首帧来自本地数据，不需要网络。',
  'state.linkDown': '未连接到本地服务',
  'state.linkDownHint': '在本机启动本地服务后会自动接上（默认 127.0.0.1:17323）。',
  'state.reconnect': '重试连接',
  'state.offlineBanner': '离线：改动会先存到本机',
  'state.dbTooNew': '本地数据由更新版本的 Notera 写入，当前版本只读打开，不会写坏数据。',
  'state.attachmentMissing': '附件缺失',
  'state.dismiss': '知道了',

  /* 无障碍 */
  'a11y.skipToSearch': '跳到搜索',
  'a11y.main': '主内容',

  /* 窗口控件 */
  'win.minimize': '最小化',
  'win.maximize': '最大化或还原',
  'win.close': '关闭',

  /* 移动端 */
  'mobile.back': '返回',
  'mobile.menu': '菜单',
  'mobile.toolbar': '格式',

  /* 兜底错误文案（绝不回显原始码） */
  'note.staleEdit': '这条笔记在别处被改动了。',
  'link.unreachable': '未连接到本地服务。',
  'link.timeout': '本地服务响应超时，可以再试一次。',
  'state.inTrash': '这条在"最近删除"里，恢复后才能继续编辑。',
  'conflict.resolvedNow': '这一条已处理好。',
  'error.fallback': '操作没有成功，可以稍后再试。',
  'error.offline': '当前离线，改动已保存在本机，恢复联网后会自动继续。',
  'error.server_unavailable': '暂时连不上你的服务器，已在本机继续工作。',
  'error.auth_required': '登录信息不可用，请在设置里重新填写口令。',
  'error.quota_full': '服务器空间不足，同步暂停，本机内容不受影响。',
  'error.protocol_mismatch': '与服务器上的数据格式不匹配，请升级后重试。',
  'error.db_too_new': '本地数据来自更新版本的 Notera，已按只读方式打开。',
  'error.conflict_needs_attention': '有几条笔记出现分歧，请到"需要处理的版本"里确认。',
  'error.sync_failed': '这一轮同步没完成，会自动重试。',
  'error.corrupt_record': '发现一条无法识别的记录，已跳过以保护其他数据。',
  'error.attachment_missing': '附件暂时不可用，正文不受影响。',
  'error.proxy_unreachable': '代理不可达，请检查设置里的代理参数。',
  'error.cert_untrusted': '服务器证书校验未通过。若确认是自签证书，可在设置里调整校验方式。',
  'error.timeout': '这一步等待太久，已中断，可以再试一次。',
  'error.transport_unreachable': '未连接到本地服务。',
  'error.stale_edit': '这条笔记在别处被改动了。',
  'error.not_found': '这条内容已经不在了。',
  'error.invalid_input': '输入的内容无法保存，请检查后重试。',
  'error.permission_denied': '系统拒绝了这次操作。',
  'error.no_account': '还没有配置同步服务器。不配置也能继续记笔记。',
  'error.multi_account_unsupported': '当前版本一台设备只支持一台同步服务器。请先停用现有的，再添加新的。',
  'error.storage': '本机存储这一步没成功。笔记仍在磁盘上，可以重试。',
  'error.constraint': '这份内容不符合本地数据的规则，没有被保存。',
  'error.bad_args': '这一步的参数没被理解，已取消，未改动任何数据。',
  'error.bad_id': '找不到这条内容的标识，列表可能已经变了。请刷新后重试。',
  'error.unknown_command': '这个操作在当前版本里不存在。',
  'error.serialize': '数据没能整理成可保存的形式，已取消，未改动任何数据。',
  'error.handler_panic': '这一步异常中止了，可以再试一次；本机数据未受影响。',
  'error.bad_device': '本机设备标识不可用，同步已暂停；笔记照常保存在本机。',
  'error.net_config': '网络出口没有配置好，这一轮同步跳过。',
  'error.invalid_account': '这台同步服务器的地址或参数不可用，请在设置里检查。',
  'error.save_failed': '设置没能保存，改动还留在这台设备上。',
  'error.proxy_credentials_pending': '代理需要账号与口令，当前版本还不能安全地保存它们。',
  'sync.root_mismatch': '这个服务器上已经是另一个 Notera 库了，已停止同步以免把两个库混在一起。请改用该库原本的路径。',
  'sync.foreign_root': '这个目录里已有别的数据，但不是本库的目录，已停止同步。请换一个根路径。',
  'sync.protocol_unreadable': '暂时读不到服务器上的协议信息，这一轮不写入。请稍后重试。',
  'slash.menu': '块类型命令面板',
  'slash.paragraph': '正文',
  'slash.paragraphHint': '普通段落，回车换行',
  'slash.heading': '标题 1',
  'slash.heading2': '标题 2',
  'slash.heading3': '标题 3',
  'slash.headingHint': '大纲层级，可被搜索命中',
  'slash.bullet': '无序列表',
  'slash.bulletHint': '一行一条，Tab 缩进',
  'slash.ordered': '有序列表',
  'slash.orderedHint': '自动编号',
  'slash.checklist': '待办事项',
  'slash.checklistHint': '点击或 Ctrl+回车 勾选',
  'slash.quote': '引用',
  'slash.quoteHint': '引用他人的段落',
  'slash.code': '代码块',
  'slash.codeHint': '保留缩进与换行，不套行内格式',
  'sync.needsCredentials': '还没有可用的登录凭据。笔记照常保存在本机，配好凭据后会自动开始同步。',
};

const TOAST_PREFIXES = ['notera.', 'error.', 'sync.', 'toast.', 'state.', 'note.', 'link.', 'settings.', 'cmd.'];

/** messageKey 归一化：容忍大小写、驼峰、点号前缀等差异。 */
export function normalizeMessageKey(key: string): string {
  let value = key.trim();
  if (value.length === 0) return 'fallback';
  for (const prefix of TOAST_PREFIXES) {
    if (value.toLowerCase().startsWith(prefix)) {
      value = value.slice(prefix.length);
      break;
    }
  }
  const snake = value
    .replace(/([a-z0-9])([A-Z])/g, '$1_$2')
    .replace(/[\s./-]+/g, '_')
    .toLowerCase();
  return snake;
}

/** 按 messageKey 取文案；命中不了用兜底，绝不显示原始码。 */
export function messageFor(key: string | null | undefined): string {
  if (!key) return MESSAGES['error.fallback'];
  if (MESSAGES[key] !== undefined) return MESSAGES[key];
  const direct = `error.${key}`;
  if (MESSAGES[direct] !== undefined) return MESSAGES[direct];
  const normalized = normalizeMessageKey(key);
  const candidate = `error.${normalized}`;
  return MESSAGES[candidate] ?? MESSAGES['error.fallback'];
}

/** 普通界面文案。缺失时返回键名本身（开发期可见，不会白屏）。 */
export function t(key: MessageKey, params?: Record<string, string | number>): string {
  const template = MESSAGES[key] ?? key;
  if (!params) return template;
  return template.replace(/\{(\w+)\}/g, (_, name: string) => {
    const value = params[name];
    return value === undefined ? `{${name}}` : String(value);
  });
}

export function hasMessage(key: MessageKey): boolean {
  return MESSAGES[key] !== undefined;
}
