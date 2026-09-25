-- 0004_indexes.sql —— 检索/列表/同步队列的索引（DATA-MODEL.md §13 视图与 §14 预算的支撑）
-- 命名规范：idx_<表>_<列>（DATA-MODEL §1）。本文件只建索引，不做任何 ALTER。

-- 列表主路径：侧栏按 (回收站标记, 文件夹) 过滤后按 updated_at 倒序（§13 v_note_list）
CREATE INDEX idx_notes_list ON notes(deleted_at, folder_id, updated_at DESC);
CREATE INDEX idx_notes_folder ON notes(folder_id);
CREATE INDEX idx_notes_updated ON notes(updated_at DESC);
CREATE INDEX idx_notes_pinned ON notes(pinned, updated_at DESC) WHERE deleted_at IS NULL;
-- 脏实体扫描（rev != sync_rev）与 tombstone 传播查询
CREATE INDEX idx_notes_dirty ON notes((rev <> sync_rev));
CREATE INDEX idx_notes_content_hash ON notes(content_hash);
CREATE INDEX idx_folders_parent ON folders(parent_id);
CREATE INDEX idx_folders_dirty ON folders((rev <> sync_rev));
-- note_revisions：base 取值（§4.4 保留窗口）与 GC 扫描
CREATE INDEX idx_revisions_note_rev ON note_revisions(note_id, rev DESC);
CREATE INDEX idx_revisions_archived ON note_revisions(note_id, archived, rev);
-- 附件生命周期（§8）
CREATE INDEX idx_attachments_local_state ON attachments(local_state);
CREATE INDEX idx_attachments_deleted ON attachments(deleted_at);
CREATE INDEX idx_note_attachments_sha ON note_attachments(sha256);
-- tombstone 不自动 GC（ADR-0006）：按 rev 序遍历远端已确认点
CREATE INDEX idx_tombstones_purged ON tombstones(entity_type, purged, rev);
-- outbox 取件与退避（§11 崩溃可恢复）
CREATE INDEX idx_outbox_take ON sync_operations(state, next_retry_at, id);
CREATE INDEX idx_outbox_entity ON sync_operations(account_id, entity_type, entity_id, state);
CREATE INDEX idx_remote_index_kind ON sync_remote_index(account_id, kind, rev);
CREATE INDEX idx_conflicts_state ON sync_conflicts(state, created_at DESC);
