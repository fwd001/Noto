"""生成 Notera 的占位图标集（合法 ICO + PNG），供 tauri-build 在 Windows 资源嵌入阶段使用。

不依赖 PIL：直接按微软 ICO 容器格式与 PNG 规范写字节。
真实品牌图标定稿后替换本文件产物即可（文件名与尺寸约定不变）。
"""
import io
import os
import struct
import zlib

OUT = r"D:/code/Notes/apps/desktop/src-tauri/icons"
os.makedirs(OUT, exist_ok=True)


def solid_png(path, size, rgba):
    """单色 PNG（非交错，8bit RGBA）。"""
    raw = b"".join(b"\x00" + bytes(rgba) * size for _ in range(size))

    def chunk(tag, data):
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(raw, 9))
    png += chunk(b"IEND", b"")
    io.open(path, "wb").write(png)
    return png


def png_to_ico_entry(png_bytes):
    """Vista+ 允许在 ICO 里直接放 PNG，体积小且被 WebView/资源管理器接受。"""
    return png_bytes


def ico(path, size, rgba):
    png = solid_png("_tmp.png", size, rgba)
    header = struct.pack("<HHH", 0, 1, 1)
    # 宽高 0 表示 256；planes/bitcount 在 PNG 载荷下按规范填 0
    w = 0 if size >= 256 else size
    h = 0 if size >= 256 else size
    # ICONDIRENTRY: w,h,colorCount,reserved,planes,bitCount,bytesInRes,imageOffset
    entry = struct.pack("<BBBBHHII", w, h, 0, 0, 1, 32, len(png), 6 + 16)
    io.open(path, "wb").write(header + entry + png)
    os.remove("_tmp.png")


# 品牌色占位：深墨蓝底 + 留白，尺寸齐 tauri 默认期望
solid_png(os.path.join(OUT, "32x32.png"), 32, (28, 36, 51, 255))
solid_png(os.path.join(OUT, "128x128.png"), 128, (28, 36, 51, 255))
solid_png(os.path.join(OUT, "icon.png"), 512, (28, 36, 51, 255))
ico(os.path.join(OUT, "icon.ico"), 256, (28, 36, 51, 255))
# 圆角遮罩版留给品牌定稿；此处保证文件存在即可
print("icons:", sorted(os.listdir(OUT)))
print("ico bytes:", os.path.getsize(os.path.join(OUT, "icon.ico")))
