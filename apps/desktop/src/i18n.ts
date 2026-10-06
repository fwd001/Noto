/**
 * 文案表（v1 只出简体中文，键为语义名）。
 * 规则：错误一律按后端给的 messageKey 查本表，查不到就显示通用兜底文案 ——
 * UI 永远不显示原始错误码、内部状态名或 HTTP 状态。
 */

export type MessageKey = string;

const MESSAGES: Record<MessageKey, string> = {
  'app.name': 'Noto',
  'app.tagline': '本地优先 · 自有服务器同步',

  /* 侧栏 */
  'sidebar.allNotes': '全部笔记',
  'sidebar.folders': '文件夹',
  // 分区小标题。侧栏现在有"导航 / 同步 / 文件夹"三块，
  // 每一块都带同样式的小标题，眼睛就不必靠"有没有边框"去猜层级
  // （用户反馈"层级乱"）。跟"文件夹"那个标题用同一套 `.section-title`。
  'sidebar.syncSection': '同步',
  'sidebar.trash': '最近删除',
  'sidebar.newFolder': '新建文件夹',
  'sidebar.rename': '重命名',
  'sidebar.moveTo': '移动到…',
  'sidebar.deleteFolder': '删除文件夹',
  // §3.2 侧栏底部那行读数：让人知道"东西在哪、有多大"。不出现协议词。
  'sidebar.libraryReadout': '本机 {notes} 篇 · {folders} 个文件夹 · {size}',
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
  'list.newNote': '新建笔记',
  'list.newFromTemplate': '按模板新建',
  'template.blank': '空白',
  'template.todo': '待办清单',
  'template.meeting': '会议记录',
  'list.dailyNote': '今天',
  'list.pin': '固定',
  'list.unpin': '取消固定',
  'list.hasAttachment': '含附件',
  'list.trashMode': '最近删除',  'list.moveToTrash': '移到最近删除',
  'list.delete': '删除',
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
  'editor.pane': '笔记正文',
  'editor.placeholder': '请输入标题和正文',
  'editor.saved': '已保存',
  'editor.saving': '保存中',
  'editor.unsaved': '未保存',
  'editor.readOnly': '只读',
  'editor.staleTitle': '这条笔记在别处被改动了',
  'editor.staleBody': '为避免覆盖，已显示对方版本。你的改动仍保留在下方，可自行取舍。',
  'editor.useMyDraft': '用我的版本继续编辑',
  'editor.discardMyDraft': '放弃我的改动',
  'editor.versionTooNew': '这条笔记由更新版本的 Noto 保存，请升级后编辑（当前可查看）。',
  'editor.unknownBlock': '暂不支持的内容（已原样保留）',
  'editor.attachmentMissing': '附件不在这台设备上，正在等待下载',
  // 本机不可用的其余各格（只有一半 / 校验不过 / 服务器那边也没有或坏了 / 连账都没有）。
  // 刻意不说"正在等待下载"：那句话只在"本机没有 + 服务器有"时才是真话。
  'editor.attachmentNotOnDevice': '这台设备上没有可用的这份附件',
  // 坏图/坏附件占位上的两个手动动作（2026-09-28 的决定：终态那一格必须给用户一个能点的东西）。
  // 界面只表达意图 —— 撤哪一半状态、要不要覆盖服务器，判断全在核心那两条命令里。
  'editor.attachmentRetry': '重试取回',
  'editor.attachmentReupload': '重新上传本机这份',
  'editor.attachmentRetryDone': '已重新排进下载队列，下一次同步会再去问服务器一次。',
  'editor.attachmentReuploadDone': '已记下你的要求，下一次同步会把本机这份覆盖上去。',
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
  'editor.insertBelow': '在下方插入块',
  'editor.dragHandle': '拖拽重排（也可用上下方向键）',
  'editor.dragHint': '按住拖动重排 · Alt+↑/↓ 也可',
  'editor.selectionBar': '选区格式',

  /* 工具条 */
  'tb.bold': '加粗',
  'tb.italic': '斜体',
  'tb.underline': '下划线',
  'tb.strike': '删除线',
  'tb.size': '文字大小',
  'tb.sizeSmall': '小',
  'tb.sizeDefault': '标准',
  'tb.sizeLarge': '大',
  'tb.sizeHuge': '特大',
  'tb.textColor': '文字颜色',
  'tb.colorDefault': '默认色',
  'tb.colorRed': '红',
  'tb.colorOrange': '橙',
  'tb.colorGreen': '绿',
  'tb.colorBlue': '蓝',
  'tb.colorViolet': '紫',
  'tb.code': '行内代码',
  'tb.highlight': '高亮',
  'tb.link': '链接',
  'tb.linkPrompt': '输入链接地址',
  'tb.linkApply': '应用',
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
  'sync.idle': '未配置同步',
  'sync.idleGoConfigure': '未配置同步 —— 点这里去设置',
  'sync.disabled': '同步已关闭',
  'settings.dangerZone': '危险操作',
  'settings.erase': '清除一切数据…',
  'settings.eraseHint': '删除本机全部笔记、文件夹、附件与同步配置，恢复到刚安装的状态。此操作不可撤销，也没有回收站。',
  'settings.eraseConfirm': '真的要删掉本机所有数据吗？建议先「备份数据库」留一份。',
  'settings.eraseCancel': '取消',
  'settings.eraseConfirmBtn': '确认删除全部',
  'settings.eraseDone': '已清除全部数据，请重启应用',
  'settings.eraseDoneDetail': '已清空 {tables} 张表的数据，回收附件约 {kb} KB。请重启应用。',
  'sync.retry': '重试',
  // §4.3「已同步 · 上一次：{时间}」。没拿到时间时这一句**根本不出现**（不写"刚刚"，那是编的）。
  'sync.lastSuccess': '上一次：{time}',
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
  'settings.credentialStoreNone': '这台设备没有系统凭据库：口令只留在这次运行的内存里，**退出后需要重新填写**一次才能继续同步。保存不会被拒绝，本地记录与搜索也不受影响。',
  'settings.credentialVolatile': '这条口令现在只在这次运行里有效（这台设备没有系统凭据库），退出后需要重填。',
  'settings.credentialGone': '这台设备上上次填的口令已经不在了（口令只活在一次运行里）。账户与服务器地址都还在，重填一次口令就能继续同步。',
  'settings.accountRootPrefix': '存储前缀',
  // §6 的 ca_bundle / pin 两档的输入格（此前下拉里选得到、却没有输入口）。
  'settings.tlsCaPem': '内网根证书（PEM）',
  'settings.tlsCaPemPlaceholder': '-----BEGIN CERTIFICATE-----（粘贴整份证书）',
  'settings.tlsCaPemSet': '已保存根证书（留空则不修改）',
  'settings.tlsCaPemHint': '用于自建 CA 或自签证书的 WebDAV。这份证书是**追加**在系统信任库之上，不会替换系统里的根。',
  'settings.tlsCaPemHintKept': '这一格留空表示不改已存的那份；要清空请先切回「跟随系统信任库」。',
  'settings.tlsPinFingerprints': '证书指纹（一行一条 sha256）',
  'settings.tlsPinPlaceholder': '例如 3a7f…（64 位十六进制）',
  'settings.tlsPinHint': '证书链校验照常做，指纹只是叠在上面的白名单；服务器换过证书后要回来补上新指纹。',
  'settings.restoreNeedsPath': '请先在上面的路径框里填要恢复的备份文件。',
  'settings.exportPathLabel': '输出位置（导出 / 备份）',
  'settings.exportScoped': '只导出指定文件夹（子树）',
  'settings.exportScopedHint': '子树包只含勾选文件夹及其子层、祖先链，不含库内其它内容，也不含无法归属到文件夹的永久删除公告 —— 因此不能用于「仅在空库时导入」的整库还原。',
  'settings.exportPathHint': '留空则由本地核心决定文件名；已存在的文件一律不覆盖',
  'settings.inputPathLabel': '输入路径（导入 / 恢复）',
  'settings.inputPathHint': '要导入或恢复的文件完整路径',
  'settings.restoreNeedsRestart': '恢复会排到下次启动时执行：替换前会先留一份当前库。',
  'settings.restoreStaged': '已排入下次启动的恢复，重启后生效。',
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
  'settings.proxyUserSet': '已设置（留空则不改）',
  'settings.accountRemoved': '已删除同步账户。笔记都还在本机，只是不再同步了。',
  'settings.removeAccount': '删除同步账户',
  'settings.removeAccountHint': '只停掉同步：删掉服务器地址与存在系统里的口令，**不删任何笔记**。想连笔记一起清掉请用下面的「清除一切数据」。',
  'settings.removeAccountConfirm': '确定删除这台同步服务器？本机笔记不受影响，之后想再同步要重新填地址与口令。',
  'settings.removeAccountYes': '确认删除账户',
  'settings.proxyPassword': '口令',
  'settings.proxyBypass': '绕过（每行一个主机/网段/*.域名）',
  'settings.save': '保存账户',
  'settings.saved': '已保存',
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
  'platform.caps_degraded': '托盘或系统级快捷键没能挂上（可能被别的应用占着）。窗口和同步都照常，设置页里那两项会显示为不支持',
  'settings.gsQuickNote': '从任何应用里新建笔记',
  'settings.gsToggleWindow': '显示 / 隐藏窗口',
  'settings.importFiles': '导入这个文件（.enex / Markdown）',
  'settings.importFilesHint': '把 Evernote 导出的 .enex 或 .md 文件的完整路径填到上面那个输入框，再点这个按钮。一个 .enex 里的多条笔记会一起进来。',
  'settings.importFilesResult': '新增 / 重复 / 失败',
  'settings.importFilesNeedsPath': '先把要导入的文件路径填到上面的输入框',
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
  'state.dbTooNew': '本地数据由更新版本的 Noto 写入，当前版本只读打开，不会写坏数据。',
  'state.dismiss': '知道了',

  /* 无障碍 */
  'a11y.skipToSearch': '跳到搜索',

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
  'error.folder_missing': '找不到默认笔记本，导入没有进行。本机的笔记不受影响，请重启应用再试。',
  'conflict.remoteNotFetched': '没能从服务器取回那一版的正文（可能这一轮同步的预算用完了，或那条记录已被清理）。本机这一版完好，可以再点一次同步，或先保留本机版。',
  'list.contendedNote': '这条在别的设备上有了分歧，等你处理',
  'error.db_too_new': '本地数据来自更新版本的 Noto，已按只读方式打开。',
  'error.conflict_needs_attention': '有几条笔记出现分歧，请到"需要处理的版本"里确认。',
  'error.sync_failed': '这一轮同步没完成，会自动重试。',
  'error.corrupt_record': '发现一条无法识别的记录，已跳过以保护其他数据。',
  'error.attachment_missing': '附件暂时不可用，正文不受影响。',
  'error.attachment_corrupt': '本机这份附件的内容和它的校验和不一致，已拒绝显示；正文不受影响，同步会尝试把它换回来。',
  // 下面三条是「重试取回 / 重新上传本机这份」这两个手动动作的失败面。它们的共同点：
  // 点下去没生效时必须说清**为什么没生效**，而不是安静地什么都不发生 —— 用户手上有
  // 一份好字节却看到"重试失败"和看到"这台设备没有可取回的对象"，是两件完全不同的事。
  'error.attachment_not_registered': '这台设备的账上没有这份附件的记录，所以没有可重试的对象。正文和其它数据没有被改动。',
  'error.nothing_to_retry': '这台设备上已经有这份附件的本机副本，不需要取回。如果屏幕上显示的内容不对，请改用「重新上传本机这份」。',
  'error.nothing_to_upload': '本机没有这份附件的完好副本（文件不在，或内容与校验和不符），不能上传。没有向服务器发送任何内容。',
  'error.too_large': '这个文件超过单个附件 32 MiB 的上限，没有添加。正文与其它附件都没被改动。',
  'error.read_failed': '本地文件没能读出来，未改动任何数据。',
  'error.no_default_folder': '这台设备上找不到默认笔记本，已停止这一步 —— 免得把笔记放进一个说不清的位置。',
  'error.bad_action': '没看懂这个选择。冲突仍留在收件箱里，双方内容都没有被覆盖。',
  'error.conflict_payload_missing': '这张卡片上"服务器那一版"的正文还没取回来，所以这一次没有替你决定。冲突仍留在收件箱里，两台的内容都没有被改动，下一轮会自动重试。',
  'error.sync_refused': '同步被暂时拒绝：本地数据处于需要保护的状态，这一轮没有做任何改动。',
  'error.sync_auth_failed':
    '登录被拒绝：服务器或代理不认这组凭据（WebDAV 的账号/应用密码，或代理的用户名/口令）。这一轮没有改动任何东西；请在设置里核对凭据后再点「立即同步」。',
  'error.sync_busy': '上一轮同步还没结束，这次不用重复点，它会自动继续。',
  'error.unknown_account': '设置里指向的服务器账户在本机没有记录。请在设置里重新保存一次服务器信息。',
  'error.proxy_unreachable': '代理不可达，请检查设置里的代理参数。',
  'error.cert_untrusted': '服务器证书校验未通过。若确认是自签证书，可在设置里调整校验方式。',
  'error.timeout': '这一步等待太久，已中断，可以再试一次。',
  'error.transport_unreachable': '未连接到本地服务。',
  'error.stale_edit': '这条笔记在别处被改动了。',
  'error.not_found': '这条内容已经不在了。',
  'error.save_dropped': '这条笔记当时还没准备好，刚才的输入没有被接受（内容没写进去）。请重新输入；如果反复出现，请先关掉这条笔记再打开。',
  'error.invalid_input': '输入的内容无法保存，请检查后重试。',
  'error.permission_denied': '系统拒绝了这次操作。',
  'error.no_account': '还没有配置同步服务器。不配置也能继续记笔记。',
  // 不可撤销的命令在命令面还有一道闸门（`confirmed` 不为真就不执行）。界面正常走不到这里，
  // 但这条码是**公开的**：没有文案时它会退成那句通用兜底，用户就分不清"删了"与"没删"。
  'error.erase_not_confirmed': '清除一切数据需要明确确认，这一次没有删除任何内容。',
  // 配好了服务器、只是这一轮拿不到口令 —— 与上一条是两件事，措辞必须分开：
  // 合成一条时用户会去翻一个明明填满了的表单（缺口 G38 选 B 之后，没有系统凭据库的
  // 平台上**每次重启**都落在这里，不是边角情况）。
  'error.sync_needs_credentials': '这台设备上现在读不到同步口令（账户与服务器地址都还在）。本机笔记照常保存，在设置里重填一次口令就能继续同步。',
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
  'error.proxy_credential_missing':
    '这台设备读不到已保存的代理凭据（系统凭据库里那条可能已被删除，或这台设备的口令只在这次运行里有效、应用重启后就没了）。这一轮同步不会带着凭据发出去 —— 请在设置里重填一次代理的用户名与口令。',
  'error.credential_too_long':
    '这条口令太长了：这台设备一条凭据最多存 **256 个 UTF-16 单元**（普通字符大约 256 个，emoji 这类会占两个）。请换一条短一些的口令。',
  'error.credential_store_failed':
    '写进系统凭据库时失败了，口令没有保存、账户也没有生效。可以再试一次；若反复失败，请把这条提示与下方详情一起给我。',
  'sync.root_mismatch': '这个服务器上已经是另一个 Noto 库了，已停止同步以免把两个库混在一起。请改用该库原本的路径。',
  'sync.foreign_root': '这个目录里已有别的数据，但不是本库的目录，已停止同步。请换一个根路径。',
  'sync.protocol_unreadable': '暂时读不到服务器上的协议信息，这一轮不写入。请稍后重试。',
  'sync.auth_failed':
    '服务器或代理不认这组凭据（WebDAV 的账号/应用密码，或代理的用户名/口令）。这一轮没有改动任何东西，待办也没丢；在设置里改对凭据后点「立即同步」就能继续。',
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
  // 键名与核心侧逐字一致（`notera-core` 的 ErrorCode 与 host 的 Toast 都发这个键）：
  // 之前它没登记，"有版本要你决定"的提示会退化成一句通用兜底文案。
  'sync.conflict_attention': '有内容在两台设备上改得不一样，需要你决定保留哪一份。冲突不会自动覆盖任何东西。',
  'sync.probeDeferred': '这次没能完成服务器能力探测，已按最保守的方式继续同步（不会覆盖你的数据），下次启动会再试一次。',
  // §11.4：让路不是错误，但也不能不说 —— "安静地不下公告"就是安静地不同步。
  'sync.leaseHeld': '另一台设备正在写入，这一轮先让它。你的改动仍在待同步队列里，稍后会自动继续。',
  'sync.applyRejected': '服务器上有一版内容没能落进这台设备（它和本机这一版撞在了同一个版本号上）。你的改动没有丢，也不会被覆盖；这一版需要下一次同步或人工处理后才能对上。',
  // 以下 8 条是核心的错误词表（`notera-core` 的 `ErrorCode::message_key()`）会发出去的键。
  // 它们原先没登记，于是这些提示全部退化成一句"操作没有成功" —— `arch-check` 的
  // hygiene:rust-message-keys-registered 现在守着这条边，漏一个就红。
  'sync.forbidden': '服务器拒绝了这次写入，本机内容没有改动。',
  'sync.precondition': '服务器上的这份内容已经变了，这一轮会重算后再写。',
  'sync.unsupported': '这台服务器不支持这项操作，已按它支持的方式继续。',
  'sync.divergence': '远端的清单与本地记录对不上，已暂停写入以保护现有数据。',
  'sync.cancelled': '这一轮同步被取消，本机内容没有改动。',
  'app.db_too_new': '本地数据来自更新版本的 Noto，已按只读方式打开。',
  'attach.missing': '附件暂时不可用，正文不受影响。',
  'attach.tooLarge': '这个文件超过单个附件 32 MiB 的上限，没有添加。正文与其它附件都没被改动。',
  'attach.empty': '这个文件是空的，没有添加。',
  'proxy.cert_untrusted': '服务器证书校验未通过。若确认是自签证书，可在设置里调整校验方式。',
  // §5 探测结果的界面表达（SYNC-PROTOCOL §5 末行：S3 必须被如实标出来）
  'settings.serverCaps': '服务器能力（首次连接与每天自动探测）',
  'sync.capsUnknown': '还没有对这台服务器做过能力探测。下一次连上网时会自动探测，并在这里告诉你结果。',
  'sync.capsProtected': '这台服务器支持并发保护（写入策略 {strategy}）：多人同时改会被服务器拦下，不会互相覆盖。',
  'sync.capsUnprotected': '这台服务器不支持条件写入（策略 S3）：靠"写完再核对"来防覆盖，存在很短的覆盖窗口。建议多台设备**串行**编辑，改完等它同步完再换设备。',
  'sync.capsProbedAt': '上次探测：{when}',
  'sync.cap.conditionalPut': '条件写入',
  'sync.cap.overwriteFMove': '不覆盖式移动',
  'sync.cap.strongEtag': '强 ETag',
  'sync.cap.depthInfinity': '递归列举',
  'sync.cap.range': '分段读取',
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

/** 全部已登记文案键。存在的理由只有一个：让测试能扫源码，找出"用了但没登记"的键。 */
export const MESSAGE_KEYS: readonly string[] = Object.keys(MESSAGES);
