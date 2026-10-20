// 简繁对照与通假字表（与 macOS 端 src/trad.rs 一致）。

import Foundation

enum Trad {
    /// 简体 -> 繁体（覆盖常用典籍用字）。
    static let s2t: [Character: Character] = {
        let pairs: [(Character, Character)] = [
            ("经", "經"), ("无", "無"), ("为", "為"), ("万", "萬"), ("观", "觀"),
            ("众", "眾"), ("门", "門"), ("谓", "謂"), ("异", "異"), ("两", "兩"),
            ("恶", "惡"), ("难", "難"), ("长", "長"), ("较", "較"), ("倾", "傾"),
            ("声", "聲"), ("后", "後"), ("随", "隨"), ("圣", "聖"), ("处", "處"),
            ("辞", "辭"), ("贤", "賢"), ("争", "爭"), ("贵", "貴"), ("货", "貨"),
            ("盗", "盜"), ("见", "見"), ("乱", "亂"), ("虚", "虛"), ("实", "實"),
            ("强", "強"), ("则", "則"), ("与", "與"), ("学", "學"), ("习", "習"),
            ("书", "書"), ("读", "讀"), ("乐", "樂"), ("爱", "愛"), ("国", "國"),
            ("义", "義"), ("礼", "禮"), ("传", "傳"), ("体", "體"), ("时", "時"),
            ("风", "風"), ("云", "雲"), ("电", "電"), ("车", "車"), ("马", "馬"),
            ("鸟", "鳥"), ("鱼", "魚"), ("龙", "龍"), ("华", "華"), ("汉", "漢"),
            ("语", "語"), ("词", "詞"), ("汇", "匯"), ("译", "譯"), ("讲", "講"),
            ("议", "議"), ("论", "論"), ("证", "證"), ("试", "試"), ("题", "題"),
            ("问", "問"), ("记", "記"), ("忆", "憶"), ("温", "溫"), ("练", "練"),
            ("复", "複"), ("关", "關"), ("于", "於"), ("还", "還"), ("这", "這"),
            ("么", "麼"), ("样", "樣"), ("个", "個"), ("们", "們"), ("来", "來"),
            ("过", "過"), ("对", "對"), ("开", "開"), ("东", "東"), ("动", "動"),
            ("进", "進"), ("远", "遠"), ("边", "邊"), ("宝", "寶"),
        ]
        return Dictionary(uniqueKeysWithValues: pairs)
    }()

    /// 内置通假字表（借字 -> 本字）。
    static let tongjia: [Character: Character] = [
        "见": "现", "知": "智", "说": "悦", "蚤": "早", "有": "又",
        "反": "返", "辩": "辨", "亡": "无", "以": "已", "锡": "赐",
        "陈": "阵", "共": "供", "干": "岸", "而": "能",
    ]

    static func traditional(_ c: Character) -> Character? {
        s2t[c]
    }

    /// 先查书内自定义表，再查内置表。
    static func tongjia(_ c: Character, bookTable: [String: String]) -> Character? {
        if let v = bookTable[String(c)]?.first { return v }
        return tongjia[c]
    }

    /// 圈号 ① ~ ⑳，超过就用 (n)。
    static func circled(_ n: Int) -> String {
        let table = Array("①②③④⑤⑥⑦⑧⑨⑩⑪⑫⑬⑭⑮⑯⑰⑱⑲⑳")
        return (1...20).contains(n) ? String(table[n - 1]) : "(\(n))"
    }
}

// MARK: - 内置古汉语 / 常用字字典（resources/dict/guhanyu.json）
// 点字查义的数据源；与 macOS 端 src/dict.rs 共用同一份 JSON 内容。

enum Dict {
    static let entries: [String: String] = load()

    /// 查一个字的释义（没有则返回 nil）。
    static func definition(of c: Character) -> String? {
        entries[String(c)]
    }

    private static func load() -> [String: String] {
        guard let url = Bundle.main.url(forResource: "guhanyu", withExtension: "json"),
              let data = try? Data(contentsOf: url),
              let d = try? JSONDecoder().decode([String: String].self, from: data) else {
            return [:]
        }
        return d
    }
}
