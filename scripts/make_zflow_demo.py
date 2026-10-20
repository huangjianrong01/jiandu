#!/usr/bin/env python3
"""生成《双行夹注流转测试》样书 JSON。

四章分别覆盖：
  章一 短注单列 —— 几条短注，验证 Z 字型成对 / 左侧留白 / 兑淡注色；
  章二 长注跨列 —— 一条 ~95 字长注，超过单列容量，验证同页跨列流转；
  章三 超长跨页 —— 一条 ~240 字长注，超过整面容量，验证 pendingNote 跨页；
  章四 注文挤正文 —— 多条中等注文与正文交替，注尾把正文推过页界。

产物：
  dist/双行夹注流转测试.json            （交付文件）
  ~/.jiandu/books/双行夹注流转测试.json（装机用）
"""
import json
import os

HOME = os.path.expanduser("~")
BOOK_DIR = os.path.join(HOME, ".jiandu", "books")
REPO = "/Users/huangjianrong/Downloads/jiandu"

# ---------------- 章一：短注单列 ----------------
ch1 = {
    "title": "短注单列",
    "paragraphs": [
        {
            "text": "道可道，非常道。名可名，非常名。",
            "notes": [
                {"at": 0, "kind": "note", "len": 1, "text": "道，路也；引申为法则、本原。"},
                {"at": 8, "kind": "note", "len": 1, "text": "名，命也，所以定形。"},
            ],
        },
        {
            "text": "无名天地之始，有名万物之母。",
            "notes": [
                {"at": 2, "kind": "note", "len": 2, "text": "始终相役，有无相生。"},
                {"at": 11, "kind": "note", "len": 2, "text": "物者，不迁之称。"},
            ],
        },
        {
            "text": "故常无欲以观其妙，常有欲以观其徼。",
            "notes": [
                {"at": 7, "kind": "note", "len": 1, "text": "妙，要眇也；微之极。"},
                {"at": 15, "kind": "note", "len": 1, "text": "徼，小道也；归要于途径。"},
            ],
        },
    ],
}

# ---------------- 章二：长注跨列（~95 字 > 单列容量） ----------------
note_cross_col = (
    "谷神者，虚而能应，灵而不测。谷以喻其虚，神以喻其灵。不死，言其长存不匮，"
    "非谓形躯之不去也。玄牝者，天地之根、万物生生之户；玄，深黑色，象其无形；"
    "牝，母也，言其能生。合而观之，道体虚而生物不竭，其机甚微，其用甚弘，"
    "读《老》者于此宜尽心焉。"
)
ch2 = {
    "title": "长注跨列",
    "paragraphs": [
        {
            "text": "谷神不死，是谓玄牝。",
            "notes": [{"at": 0, "kind": "note", "len": 2, "text": note_cross_col}],
        }
    ],
}

# ---------------- 章三：超长注跨页（~240 字 > 整面容量） ----------------
note_cross_page = (
    "大道泛兮，其可左右。泛者，广被而无偏；左右者，无适而非道之流行。"
    "万物恃之以生，而道不辞其劳；功成而不名有，养万物而不为主。"
    "可名于小，可名于大：至于芥纳须弥，细入无间，亦道也。"
    "以其不自生，故能长生——不自生则无死地，此道之所以常存。"
    "是以圣人后其身而身先，外其身而身存；非以其无私邪，故能成其私。"
    "此言道之无不在，而体道者当以谦退自处。"
    "杨子曰：道无形之相，无匹之伦，包天地而究万物，其大无外，其小无内。"
    "严君平曰：道之为物，无形无状，不存不亡，欲谓之有而不可见，"
    "欲谓之无而物自成。道德之旨，于此章视其大者，小大由其分量自取，"
    "要皆道之散见也。"
)
ch3 = {
    "title": "超长注跨页",
    "paragraphs": [
        {
            "text": "大道泛兮，其可左右。",
            "notes": [{"at": 0, "kind": "note", "len": 2, "text": note_cross_page}],
        }
    ],
}

# ---------------- 章四：注文挤正文（注尾把正文推过页界） ----------------
ch4 = {
    "title": "注文挤正文",
    "paragraphs": [
        {
            "text": "上善若水，水善利万物而不争。",
            "notes": [
                {"at": 2, "kind": "note", "len": 1, "text": "水，柔弱而能攻坚强，随器而成形。"},
            ],
        },
        {
            "text": "处众之所恶，故几于道。",
            "notes": [
                {"at": 0, "kind": "note", "len": 1, "text": "众人趋高，水独就下；道亦然。"},
            ],
        },
        {
            "text": "持而盈之，不如其已。",
            "notes": [
                {"at": 0, "kind": "note", "len": 1, "text": "满则覆，锐则挫，此天之道。"},
            ],
        },
        {
            "text": "揣而锐之，不可长保。",
            "notes": [
                {"at": 0, "kind": "note", "len": 1, "text": "锐，苛也；长保则伤于早折。"},
            ],
        },
        {
            "text": "金玉满堂，莫之能守。",
            "notes": [
                {"at": 0, "kind": "note", "len": 1, "text": "富而骄，自遗其咎；守富难于创业。"},
            ],
        },
        {
            "text": "富贵而骄，自遗其咎。",
            "notes": [
                {"at": 0, "kind": "note", "len": 1, "text": "骄，矜也；咎，殃也。古人三致意焉。"},
                {"at": 8, "kind": "note", "len": 1,
                 "text": "遗，读如馈遗之遗，自留之也；言不以为意而祸忽至。"
                         "积财可以耀一时，积德可以诒百世。"
                         "观秦之奢、金之侈，皆以富贵而骄，卒不能保其所以富。"
                         "此《易》所谓「满则覆」之象，危惧之戒也。"},
            ],
        },
    ],
}

book = {
    "title": "双行夹注流转测试",
    "author": "笺读",
    "cover": {
        "title": "双行夹注流转测试",
        "subtitle": "单列 · 跨列 · 跨页 渲染验证",
        "seal": "笺读",
        "accent": 0x7A5C1E,
    },
    "tongjia": {},
    "toc": [
        {"title": ch["title"], "chapter": i, "level": 1} for i, ch in enumerate([ch1, ch2, ch3, ch4])
    ],
    "chapters": [ch1, ch2, ch3, ch4],
}


def n(s: str) -> int:
    return len(s)


print("注文字数：")
print("  章一 6 条短注：", [n(x["text"]) for p in ch1["paragraphs"] for x in p["notes"]])
print("  章二 长注跨列：", n(note_cross_col), "字 →", (n(note_cross_col) + 1) // 2, "小行")
print("  章三 超长跨页：", n(note_cross_page), "字 →", (n(note_cross_page) + 1) // 2, "小行")
print("  章四 末条中长注：", n(ch4["paragraphs"][-1]["notes"][-1]["text"]), "字")

# 写出：交付文件 + 装机文件
os.makedirs(os.path.join(REPO, "dist"), exist_ok=True)
os.makedirs(BOOK_DIR, exist_ok=True)
target = os.path.join(BOOK_DIR, "双行夹注流转测试.json")
with open(target, "w", encoding="utf-8") as f:
    json.dump(book, f, ensure_ascii=False, indent=1)
with open(os.path.join(REPO, "dist", "双行夹注流转测试.json"), "w", encoding="utf-8") as f:
    json.dump(book, f, ensure_ascii=False, indent=1)
print("已写：", target)

# 登记进 library.json（保留原有条目，进度置 0）
lib_path = os.path.join(HOME, ".jiandu", "library.json")
lib = json.load(open(lib_path, encoding="utf-8"))
lib["books"] = [b for b in lib["books"] if "双行夹注流转测试" not in b["path"]]
lib["books"].append({
    "path": target,
    "title": book["title"],
    "author": book["author"],
    "accent": book["cover"]["accent"],
    "seal": book["cover"]["seal"],
    "last_page": 0,
    "total_pages": 0,
    "script": "ancient",
    "duplex": "single",
    "last_read": 0,
    "bookmarks": [],
    "notes": {},
    "underlines": [],
})
json.dump(lib, open(lib_path, "w", encoding="utf-8"), ensure_ascii=False, indent=1)
print("已登记 library.json，共", len(lib["books"]), "本")
