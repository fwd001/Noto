-- 0005_views.sql —— 常用视图（DATA-MODEL.md §13 逐字落地）
-- 列表查询走投影，**禁止 SELECT doc**（§13 首行注释：避免正文溢出页拖慢）。

-- 侧栏列表：不读 doc，避免正文溢出页拖慢
CREATE VIEW v_note_list AS
SELECT n.id, n.folder_id, f.name AS folder_name, n.title, n.summary, n.pinned,
       n.updated_at, n.char_count, n.has_attachment,
       (n.rev <> n.sync_rev) AS dirty
FROM notes n JOIN folders f ON f.id = n.folder_id
WHERE n.deleted_at IS NULL;

CREATE VIEW v_trash AS
SELECT id, title, deleted_at,
       CAST(julianday('now') - julianday(deleted_at) AS INTEGER) AS days_left
FROM notes WHERE deleted_at IS NOT NULL;

-- 文件夹树：递归 CTE，path 为派生展示值，绝不参与同步语义
CREATE VIEW v_folder_tree AS
WITH RECURSIVE tree(id, parent_id, name, depth, path) AS (
  SELECT id, parent_id, name, 0, '/' || name FROM folders
    WHERE parent_id IS NULL AND deleted_at IS NULL
  UNION ALL
  SELECT f.id, f.parent_id, f.name, t.depth + 1, t.path || '/' || f.name
  FROM folders f JOIN tree t ON f.parent_id = t.id WHERE f.deleted_at IS NULL
)
SELECT * FROM tree;

-- 仓库自身的健康视图（诊断/verify 用；纯派生，不参与同步语义）
CREATE VIEW v_dirty_entities AS
SELECT 'note' AS entity_type, id AS entity_id, rev, sync_rev, remote_rev, content_hash,
       deleted_at, purged_at
FROM notes WHERE rev <> sync_rev
UNION ALL
SELECT 'folder', id, rev, sync_rev, remote_rev, content_hash, deleted_at, purged_at
FROM folders WHERE rev <> sync_rev;
