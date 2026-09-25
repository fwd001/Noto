#!/usr/bin/env python3
"""Phase 0 evidence: 清单（manifest）与记录尺寸实测。

结论用于 SYNC-PROTOCOL §4.2 / ADR-0004（两段式清单的尺寸依据）。
输出与 docs/evidence/manifest-sizes.txt 一致；重跑可复现（固定随机种子）。
"""
import json, gzip, uuid, random, hashlib, base64

random.seed(11)


def uid():
    """确定性 UUID 形态字符串。

    不能用 uuid.uuid4()：它走 os.urandom，不受 random.seed 控制，
    会让"可复现"变成假话（实测两次跑出 1339.9 / 1340.1 KiB）。
    """
    h = "".join(random.choice("0123456789abcdef") for _ in range(32))
    return f"{h[0:8]}-{h[8:12]}-7{h[13:16]}-8{h[16:19]}-{h[19:32]}"


def entry(style):
    """一条清单条目。style 控制字段取舍。"""
    raw = bytes.fromhex("".join(random.choice("0123456789abcdef") for _ in range(32)))
    full = hashlib.sha256(raw).hexdigest()
    h = full if style == 'A' else (full[:12] if style == 'B' else
        base64.urlsafe_b64encode(hashlib.sha256(raw).digest()).decode().rstrip('='))
    d = {"i": uid(), "r": random.randint(1, 999), "h": h, "s": random.randint(200, 9000)}
    if style == 'B':
        d["t"] = "n"
    if style == 'C':
        d["d"] = None
        d["p"] = False
    return d


def manifest(n, style):
    return {"protocol": 1, "root_id": uid(), "seq": n * 3,
            "generated_at": "2026-09-25T09:00:00.000Z", "generated_by": "dev-01",
            "software": "notera 0.1.0", "counts": {"note": n},
            "entries": [entry(style) for _ in range(n)]}


def size(n, style):
    raw = json.dumps(manifest(n, style), separators=(',', ':')).encode()
    gz = gzip.compress(raw, 9)
    return len(raw), len(gz)


print("== 清单条目编码方案对比（5000 条） ==")
for style, desc in [('A', 'full sha256 hex + size'), ('B', '12hex + kind tag（选定）'), ('C', 'base64url(43) + deleted/purged')]:
    r, g = size(5000, style)
    r2, g2 = size(20000, style)
    print(f"  {desc:34} 5000条 raw {r/1024:7.1f} KiB gzip {g/1024:7.1f} KiB ({r//5000} B/条) | 20000条 gzip {g2/1024:7.1f} KiB")

print("\n== 选定形态 {i,r,h,s,t} 的规模曲线 ==")
for n in (500, 2000, 5000, 20000, 50000):
    r, g = size(n, 'B')
    print(f"  n={n:>6}  raw {r/1024:8.1f} KiB  gzip {g/1024:8.1f} KiB")

print("\n== 两段式：变更窗口 与 基线分段 ==")
for k in (100, 200, 500):
    r, g = size(k, 'B')
    print(f"  窗口 {k:>4} 条   raw {r/1024:6.1f} KiB  gzip {g/1024:6.1f} KiB")
for seg in (1000, 2000, 4000):
    r, g = size(seg, 'B')
    print(f"  分段 {seg:>4} 条  raw {r/1024:6.1f} KiB  gzip {g/1024:6.1f} KiB")

print("\n== 单条记录信封（含 500 字中文正文的笔记） ==")
doc = {"v": 1, "content": [{"id": uid().replace("-","")[:8], "type": "paragraph",
        "content": [{"text": "这是一条中文笔记正文，用来估算单条记录上传体积。" * 10}]}]}
env = {"protocol": 1, "kind": "note", "id": uid(), "rev": 7, "sync_rev": 6,
       "hash": "sha256:" + hashlib.sha256(json.dumps(doc).encode()).hexdigest(),
       "updated_at": "2026-09-25T09:00:00.000Z", "device": "dev-01",
       "deleted_at": None, "purged": False, "enc": {"alg": "none", "hash_alg": "sha256"},
       "payload": doc, "ct": None}
b = json.dumps(env, separators=(',', ':'), ensure_ascii=False).encode()
print(f"  raw {len(b)} B  gzip {len(gzip.compress(b, 9))} B")

print("\n== 结论 ==")
print("  1. 全量清单每轮重写不可接受：5000 条 gzip 182 KiB，且随库线性增长")
print("  2. 两段式后每轮重写量 = 窗口 200 条 ≈ 7.5 KiB gzip，与库规模解耦")
print("  3. hash 截到 12 hex 比 base64url(43) 压缩后更小（hex 更规则），故清单内用 12hex，记录内存全量")
print("  4. 单条笔记记录 gzip 后数百字节量级，是改动轮的主要上行成本")
