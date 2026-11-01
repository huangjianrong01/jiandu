#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
生成 iPad 端内置公版古籍资源：
  - 3 本带真实文本层（宋体嵌入）的 PDF，打开即可被 PDFKit 提取原文→笺校。
  - 3 张宣纸黄古籍风封面 PNG。
  - builtin_books.json 清单（书名/作者/简介/封面）。

所有文本均为进入公有领域的中国古代典籍，无版权争议。
运行：python3 tools/generate_builtin_assets.py
依赖：fpdf2（文本 PDF）、Pillow（封面图），均需 pip 安装。
"""

import os
import json
from fpdf import FPDF
from PIL import Image, ImageDraw, ImageFont

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RES = os.path.join(ROOT, "JianDu", "Resources", "BuiltinBooks")
os.makedirs(RES, exist_ok=True)

SONG = "/System/Library/Fonts/Supplemental/Songti.ttc"
# fpdf2 默认会挑 TTC 的第 0 个字重脸（Songti SC Black），它缺 8 个生僻字；
# 抽出 Regular 脸（含全字）存为独立 TTF，保证千字文等生僻字不缺形。
import tempfile
_SONG_REGULAR = None
def _song_regular_ttf():
    global _SONG_REGULAR
    if _SONG_REGULAR and os.path.exists(_SONG_REGULAR):
        return _SONG_REGULAR
    from fontTools.ttLib import TTCollection
    ttc = TTCollection(SONG)
    # 选「Songti SC Regular」脸（实测含全部生僻字）。
    target = None
    for f in ttc.fonts:
        if f["name"].getDebugName(4) == "Songti SC Regular":
            target = f
            break
    if target is None:
        target = ttc.fonts[0]
    fd, path = tempfile.mkstemp(suffix=".ttf", prefix="song_reg_")
    os.close(fd)
    target.save(path)
    _SONG_REGULAR = path
    return path

COVER_W, COVER_H = 600, 840  # 书封比例 ~ 1:1.4

# ── 公版文本 ──────────────────────────────────────────────
TEXTS = {
    "daodejing": {
        "title": "道德经",
        "author": "〔春秋〕老子",
        "intro": "道家经典，五千言分道经与德经，论道法自然、无为而治。",
        "chapters": [
            ("第一章", "道可道，非常道。名可名，非常名。无名天地之始；有名万物之母。"
                       "故常无欲，以观其妙；常有欲，以观其徼。此两者同出而异名，同谓之玄。"
                       "玄之又玄，众妙之门。"),
            ("第八章", "上善若水。水善利万物而不争，处众人之所恶，故几于道。"
                       "居善地，心善渊，与善仁，言善信，政善治，事善能，动善时。"
                       "夫唯不争，故无尤。"),
            ("第二十五章", "有物混成，先天地生。寂兮寥兮，独立而不改，周行而不殆，可以为天地母。"
                           "吾不知其名，强字之曰道，强为之名曰大。大曰逝，逝曰远，远曰反。"
                           "故道大，天大，地大，人亦大。域中有四大，而人居其一焉。"
                           "人法地，地法天，天法道，道法自然。"),
            ("第三十三章", "知人者智，自知者明。胜人者有力，自胜者强。知足者富，强行者有志。"
                           "不失其所者久，死而不亡者寿。"),
            ("第八十一章", "信言不美，美言不信。善者不辩，辩者不善。知者不博，博者不知。"
                           "圣人不积，既以为人己愈有，既以与人己愈多。天之道，利而不害；"
                           "圣人之道，为而不争。"),
        ],
    },
    "qianziwen": {
        "title": "千字文",
        "author": "〔南朝〕周兴嗣",
        "intro": "四字韵文一千字，识字蒙学经典，天地人文无所不包。",
        "chapters": [
            ("全文", "天地玄黄，宇宙洪荒。日月盈昃，辰宿列张。寒来暑往，秋收冬藏。"
                      "闰余成岁，律吕调阳。云腾致雨，露结为霜。金生丽水，玉出昆冈。"
                      "剑号巨阙，珠称夜光。果珍李柰，菜重芥姜。海咸河淡，鳞潜羽翔。"
                      "龙师火帝，鸟官人皇。始制文字，乃服衣裳。推位让国，有虞陶唐。"
                      "吊民伐罪，周发殷汤。坐朝问道，垂拱平章。爱育黎首，臣伏戎羌。"
                      "遐迩一体，率宾归王。鸣凤在竹，白驹食场。化被草木，赖及万方。"
                      "盖此身发，四大五常。恭惟鞠养，岂敢毁伤。女慕贞洁，男效才良。"
                      "知过必改，得能莫忘。罔谈彼短，靡恃己长。信使可覆，器欲难量。"
                      "墨悲丝染，诗赞羔羊。景行维贤，克念作圣。德建名立，形端表正。"
                      "空谷传声，虚堂习听。祸因恶积，福缘善庆。尺璧非宝，寸阴是竞。"
                      "资父事君，曰严与敬。孝当竭力，忠则尽命。临深履薄，夙兴温凊。"
                      "似兰斯馨，如松之盛。川流不息，渊澄取映。容止若思，言辞安定。"
                      "笃初诚美，慎终宜令。荣业所基，籍甚无竟。学优登仕，摄职从政。"
                      "存以甘棠，去而益咏。乐殊贵贱，礼别尊卑。上和下睦，夫唱妇随。"
                      "外受傅训，入奉母仪。诸姑伯叔，犹子比儿。孔怀兄弟，同气连枝。"
                      "交友投分，切磨箴规。仁慈隐恻，造次弗离。节义廉退，颠沛匪亏。"
                      "性静情逸，心动神疲。守真志满，逐物意移。坚持雅操，好爵自縻。"
                      "都邑华夏，东西二京。背邙面洛，浮渭据泾。宫殿盘郁，楼观飞惊。"
                      "图写禽兽，画彩仙灵。丙舍旁启，甲帐对楹。肆筵设席，鼓瑟吹笙。"
                      "升阶纳陛，弁转疑星。右通广内，左达承明。既集坟典，亦聚群英。"
                      "杜稿钟隶，漆书壁经。府罗将相，路侠槐卿。户封八县，家给千兵。"
                      "高冠陪辇，驱毂振缨。世禄侈富，车驾肥轻。策功茂实，勒碑刻铭。"
                      "磻溪伊尹，佐时阿衡。奄宅曲阜，微旦孰营。桓公匡合，济弱扶倾。"
                      "绮回汉惠，说感武丁。俊乂密勿，多士寔宁。晋楚更霸，赵魏困横。"
                      "假途灭虢，践土会盟。何遵约法，韩弊烦刑。起翦颇牧，用军最精。"
                      "宣威沙漠，驰誉丹青。九州禹迹，百郡秦并。岳宗泰岱，禅主云亭。"
                      "雁门紫塞，鸡田赤城。昆池碣石，钜野洞庭。旷远绵邈，岩岫杳冥。"
                      "治本于农，务兹稼穑。俶载南亩，我艺黍稷。税熟贡新，劝赏黜陟。"
                      "孟轲敦素，史鱼秉直。庶几中庸，劳谦谨敕。聆音察理，鉴貌辨色。"
                      "贻厥嘉猷，勉其祗植。省躬讥诫，宠增抗极。殆辱近耻，林皋幸即。"
                      "两疏见机，解组谁逼。索居闲处，沉默寂寥。求古寻论，散虑逍遥。"
                      "欣奏累遣，戚谢欢招。渠荷的历，园莽抽条。枇杷晚翠，梧桐蚤凋。"
                      "陈根委翳，落叶飘摇。游鹍独运，凌摩绛霄。耽读玩市，寓目囊箱。"
                      "易輶攸畏，属耳垣墙。具膳餐饭，适口充肠。饱饫烹宰，饥厌糟糠。"
                      "亲戚故旧，老少异粮。妾御绩纺，侍巾帷房。纨扇圆絜，银烛炜煌。"
                      "昼眠夕寐，蓝笋象床。弦歌酒宴，接杯举觞。矫手顿足，悦豫且康。"
                      "嫡后嗣续，祭祀烝尝。稽颡再拜，悚惧恐惶。笺牒简要，顾答审详。"
                      "骸垢想浴，执热愿凉。驴骡犊特，骇跃超骧。诛斩贼盗，捕获叛亡。"
                      "布射僚丸，嵇琴阮啸。恬笔伦纸，钧巧任钓。释纷利俗，并皆佳妙。"
                      "毛施淑姿，工颦妍笑。年矢每催，曦晖朗曜。璇玑悬斡，晦魄环照。"
                      "指薪修祜，永绥吉劭。矩步引领，俯仰廊庙。束带矜庄，徘徊瞻眺。"
                      "孤陋寡闻，愚蒙等诮。谓语助者，焉哉乎也。"),
        ],
    },
    "lunyu": {
        "title": "论语",
        "author": "〔先秦〕孔子弟子及再传弟子",
        "intro": "儒门语录，记孔子与门人言行，立仁礼之教，垂范两千载。",
        "chapters": [
            ("学而第一（选）", "子曰：「学而时习之，不亦说乎？有朋自远方来，不亦乐乎？"
                              "人不知而不愠，不亦君子乎？」"
                              "有子曰：「其为人也孝弟，而好犯上者，鲜矣；"
                              "不好犯上，而好作乱者，未之有也。君子务本，本立而道生。"
                              "孝弟也者，其为仁之本与！」"),
            ("为政第二（选）", "子曰：「为政以德，譬如北辰，居其所而众星共之。」"
                              "子曰：「吾十有五而志于学，三十而立，四十而不惑，"
                              "五十而知天命，六十而耳顺，七十而从心所欲，不逾矩。」"),
            ("雍也第六（选）", "子曰：「贤哉回也！一箪食，一瓢饮，在陋巷，人不堪其忧，"
                              "回也不改其乐。贤哉回也！」"),
            ("述而第七（选）", "子曰：「默而识之，学而不厌，诲人不倦，何有于我哉？」"
                              "子曰：「饭疏食饮水，曲肱而枕之，乐亦在其中矣。"
                              "不义而富且贵，于我如浮云。」"),
            ("子罕第九（选）", "子在川上曰：「逝者如斯夫！不舍昼夜。」"
                              "子曰：「三军可夺帅也，匹夫不可夺志也。」"),
        ],
    },
}


def make_pdf(key: str, meta: dict, out_path: str):
    pdf = FPDF(format="A4")
    pdf.set_auto_page_break(auto=True, margin=22)
    # 只用常规字重（Regular 脸），避免 TTC 粗体子集缺生僻字（标题靠字号区分）。
    pdf.add_font("Song", "", _song_regular_ttf())
    pdf.set_margins(20, 20, 20)

    # 扉页
    pdf.add_page()
    pdf.set_font("Song", "", 30)
    pdf.ln(40)
    pdf.cell(0, 20, meta["title"], align="C")
    pdf.ln(24)
    pdf.set_font("Song", "", 15)
    pdf.cell(0, 12, meta["author"], align="C")
    pdf.ln(16)
    pdf.set_font("Song", "", 11)
    pdf.set_text_color(120)
    pdf.multi_cell(0, 7, meta["intro"], align="C")
    pdf.set_text_color(0)

    # 正文
    pdf.set_font("Song", "", 16)
    line_h = 9
    for ctitle, body in meta["chapters"]:
        pdf.add_page()
        pdf.set_font("Song", "", 18)
        pdf.cell(0, 12, ctitle)
        pdf.ln(14)
        pdf.set_font("Song", "", 16)
        # 按字符手工折行（CJK 无空格）；每行约 22 字
        chars = list(body)
        per = 22
        for i in range(0, len(chars), per):
            pdf.cell(0, line_h, "".join(chars[i:i + per]))
            pdf.ln(line_h)
        pdf.ln(4)
    pdf.output(out_path)
    print("PDF ->", out_path, round(os.path.getsize(out_path) / 1024, 1), "KB")


def make_cover(key: str, meta: dict, out_path: str):
    img = Image.new("RGB", (COVER_W, COVER_H), (247, 240, 220))  # 宣纸黄
    d = ImageDraw.Draw(img)
    # 双线边框
    d.rectangle([18, 18, COVER_W - 18, COVER_H - 18], outline=(120, 90, 60), width=3)
    d.rectangle([26, 26, COVER_W - 26, COVER_H - 26], outline=(120, 90, 60), width=1)
    font_title = ImageFont.truetype(SONG, 64, index=6)  # Regular 脸
    font_sub = ImageFont.truetype(SONG, 26, index=6)
    font_seal = ImageFont.truetype(SONG, 30, index=6)
    # 竖排书名（从右向左）
    title = meta["title"]
    x = COVER_W - 150
    y = 200
    for ch in title:
        d.text((x, y), ch, font=font_title, fill=(40, 30, 20))
        y += 72
    # 作者
    d.text((70, COVER_H - 170), meta["author"], font=font_sub, fill=(90, 70, 50))
    # 朱砂印章
    d.rectangle([70, COVER_H - 120, 120, COVER_H - 70], outline=(176, 52, 44), width=3)
    d.text((78, COVER_H - 112), "笺", font=font_seal, fill=(176, 52, 44))
    img.save(out_path)
    print("COVER->", out_path, round(os.path.getsize(out_path) / 1024, 1), "KB")


def main():
    books = []
    for key, meta in TEXTS.items():
        pdf_name = f"{key}.pdf"
        cover_name = f"builtin_{key}_cover.png"
        make_pdf(key, meta, os.path.join(RES, pdf_name))
        make_cover(key, meta, os.path.join(RES, cover_name))
        books.append({
            "key": key,
            "title": meta["title"],
            "author": meta["author"],
            "intro": meta["intro"],
            "resource": pdf_name,
            "cover": cover_name,
            "kind": "pdf",
        })
    manifest = {"version": 1, "books": books}
    with open(os.path.join(RES, "builtin_books.json"), "w", encoding="utf-8") as f:
        json.dump(manifest, f, ensure_ascii=False, indent=2)
    print("MANIFEST -> builtin_books.json with", len(books), "books")


if __name__ == "__main__":
    main()
