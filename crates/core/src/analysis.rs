//! **古籍文本量化分析**（v1.81.0 · P2 模块五）。
//!
//! 一句话定位：**这不是语法分析，是数数**。
//! 中文不分词、文言无词性，硬上 NLP 只会得到一堆似是而非的数字。这里只
//! 做三件人力也能做、但人手做不快的事：
//!
//!   1. **字词**——总字数 / 不重复字数 / 多样性 / 高频词组 / 生僻字·异体字；
//!   2. **句读**——「读」（，、：；）与「句」（。）各多少枚，平均每句几字；
//!   3. **文风**——文言虚词密度 vs 白话代词密度，配上书名号 / 专名号处数，
//!      给一个「偏文言 / 偏白话 / 引经据典 / 文白相杂」的标签。
//!
//! ## 三条必须守住的自律
//!
//! * **只数汉字**：标点、空白、西文、夹注注文都不进「总字数」——否则《道德
//!   经》五千言会被算成六千，而用户心里那五千是版本常识。
//! * **不引分词库**：高频词走 **2 字 / 3 字 n-gram + 虚词过滤 + 冗余剪枝**，
//!   零依赖、零词典加载，且颗粒度正好合「古风标签云」的胃口。
//! * **原文一字不动**：本模块只读不写，`analyzer` 拿到的永远是副本或引用，
//!   不参与任何转换 / 归一（繁简是渲染层的事，见 `trad`）。
//!
//! ## 为什么不直接给一个 `analyze(&Book)`
//!
//! 整本百万字的书在 core 里是**骨架模式**（正文在 SQLite 里，`chapters` 是
//! 空壳，见 `Book::chapter_chars`）。若在这里遍历 `book.chapters`，拿到的
//! 会是空的。故本模块只提供**累加器** [`Analyzer`]：壳子按「本书 / 本章 /
//! 选区 / 单卡」四种口径自己喂段落进来，算完调 [`Analyzer::finish`]。
//! 这样也顺带解决了「全本分析会不会撑爆内存」——喂多少算多少，配合
//! [`crate::mem::breathe`] 随时可以让一让。

use crate::dict;
use crate::gloss::{jodu_of, Jodu, PunctMode};
use crate::trad;
use crate::types::{Note, NoteKind};
use std::collections::HashMap;

// ── 字 ────────────────────────────────────────────────────────────────────

/// **汉字判据**（比 [`crate::west::is_cjk`] 严一档）。
///
/// 只要**汉字**，不要假名 / 谚文 / 部首符号 / 竖排标点：
///   * 部首扩展区（U+2E80–U+2FFF）是「符号」，不是字；
///   * 兼容区（U+F900–U+FAFF）与扩展 B 区以后（U+20000+）**要**——那些
///     正是古籍里最该被点出来的异写；
///   * `〇`（U+3007）是汉字数字，`々`（U+3005）是叠字符号，一并收。
pub fn is_han(c: char) -> bool {
    let u = c as u32;
    matches!(u,
        0x3005..=0x3007      // 々 〆 〇
        | 0x3400..=0x4DBF    // 扩展 A
        | 0x4E00..=0x9FFF    // 基本区
        | 0xF900..=0xFAFF    // 兼容区
        | 0x20000..=0x323AF  // 扩展 B ~ F
    )
}

/// **异体字判据**：一个字「之所以被单独编码，只是因为它是别字的另一种写法」。
///
/// 三条，命中任一条即算（可重复、可解释，绝不靠机器学习猜）：
///   1. Unicode **兼容区** U+F900–U+FAFF / 兼容补充区 U+2F800–U+2FA1F
///      ——标准自己就写明这是为兼容旧字符集而单列的异体；
///   2. **扩展 B 区及以后**（U+20000+）——罕用形，绝大多数是异写；
///   3. 简繁对照表能把它归一到**一个常用字**（`說 → 说`、`峯 → 峰`）
///      ——「冷僻写法 + 有通行正形」，正是读者要被提醒的那种字。
///
/// 注意它与「生僻字」是**包含关系**：异体 ⊆ 生僻。UI 里应写成
/// 「生僻 N 字，其中异体 M 字」，不要并列成两个不相干的数。
pub fn is_variant_char(c: char) -> bool {
    let u = c as u32;
    if (0xF900..=0xFAFF).contains(&u) || (0x2F800..=0x2FA1F).contains(&u) {
        return true;
    }
    if (0x20000..=0x323AF).contains(&u) {
        return true;
    }
    for alt in [trad::simplified_of(c), trad::traditional_of(c)]
        .into_iter()
        .flatten()
    {
        if alt != c && dict::is_common_char(alt) {
            return true;
        }
    }
    false
}

/// **生僻字判据**：常用字表（GB2312 一级字 + 其繁体形）之外。
///
/// 复用 [`dict::is_common_char`]——与自动注音**同一把尺子**：会给你注音的
/// 那批字，就是这里数的「生僻字」。两个功能口径分裂是灾难（一处说生僻、
/// 一处不注音），故绝不另起一份表。
pub fn is_rare_char(c: char) -> bool {
    is_han(c) && !dict::is_common_char(c)
}

// ── 词（n-gram）────────────────────────────────────────────────────────────

/// **停用字**：文言里只起语法作用、单独成词毫无信息量的字。
///
/// 一个 n-gram 里只要含其中任一字就整条丢弃——于是「之曰」「也者」这类
/// 高频噪声被滤掉，留下「天下」「君子」「道德」这些真词。
///
/// 注意「不 / 有 / 无 / 人」**不在**里头：它们参与构词（不仁、有为、无人），
/// 一刀切会把「不亦乐乎」整条砍没。
const STOP: &[char] = &[
    '之', '乎', '者', '也', '矣', '焉', '哉', '兮', '曰', '蓋', '盖', '夫', '斯', '乃', '遂',
    '故', '豈', '岂', '孰', '曷', '盍', '茲', '兹', '厥', '暨', '爾', '尔', '汝', '吾', '予',
    '則', '则', '與', '与', '為', '为', '所', '於', '于', '以', '而', '其', '是', '此', '彼',
];

/// 一个 n-gram 至少要出现几次才配进标签云。
///
/// 1 次就进榜的话，长文里全是「孤例」，标签云变成乱码墙。
const MIN_GRAM_HITS: usize = 2;

/// 标签云最多给几条。
const TOP_N: usize = 12;

// ── 文风 ──────────────────────────────────────────────────────────────────

/// **文言虚词**：之乎者也矣焉哉那一路。密度高 = 文言。
const CLASSICAL: &[char] = &[
    '之', '乎', '者', '也', '矣', '焉', '哉', '兮', '曰', '蓋', '夫', '斯', '乃', '遂', '故',
    '豈', '孰', '曷', '盍', '茲', '厥', '暨', '爾', '汝', '吾', '予',
];

/// **白话标记**：的了着吗呢那一路。密度高 = 白话。
const VERNACULAR: &[char] = &[
    '的', '了', '着', '吗', '呢', '吧', '把', '被', '们', '这', '那', '呀', '哦', '啦', '嘛',
    '呗', '很', '就',
];

/// 字数少于此数就不给文风标签。
///
/// 定 20 而不是更高：本模块的主战场是**卡片匣里的一句摘录**，摘录通常
/// 只有十几二十字；定到 30 会让八成卡片都显示「字少难断」，等于没做。
/// 20 字约合文言一句半，密度（每百字）在这个长度上已经能定方向——
/// 剩下那点噪声，靠「只给四选一的粗标签」吸收掉。
const MIN_LABEL_CHARS: usize = 20;

/// **记言标记**：引语动词。密度高 = 语录体（《论语》《世说》那一路）。
///
/// 只收「说」这个动作本身，不收「曰」以外的称谓——「谓」「云」在先秦是
/// 引语动词，「问 / 对 / 答」是问答体的骨架，凑在一起才判得出「记言还是
/// 记事」：一部《资治通鉴》与一部《论语》在这条上差着一个量级。
const SPEECH: &[char] = &['曰', '云', '謂', '谓', '問', '问', '對', '对', '答'];

/// 两个密度差到几倍才算「偏」一边（1.6 倍：肉眼能感到的差别）。
const LABEL_RATIO: f32 = 1.6;

/// 算「字汇覆盖率」取前多少个常用字。
///
/// 100 是文言的经验值：《史》《汉》一类叙事文，前一百字就能盖住六成以上
/// 篇幅；而骈文、辞赋为了铺陈，用字散得多，覆盖率明显更低。**这个数比
/// `unique/total` 有用得多**——后者在长文本上必然趋近 0，什么都说明不了。
const COVER_TOP: usize = 100;

/// **十天干** / **十二地支**：相邻「干 + 支」即一枚干支纪年（甲子、乙丑…）。
const GAN: &[char] = &['甲', '乙', '丙', '丁', '戊', '己', '庚', '辛', '壬', '癸'];
const ZHI: &[char] = &[
    '子', '丑', '寅', '卯', '辰', '巳', '午', '未', '申', '酉', '戌', '亥',
];
/// 汉字数字（判「X年」这种纪年写法用）。
const NUM_HAN: &[char] = &[
    '〇', '零', '一', '二', '三', '四', '五', '六', '七', '八', '九', '十', '百', '千', '萬', '万',
    '兩', '两',
];

/// 多短的句子不算「俪句」。
///
/// 「三言对三言」多半是巧合（「道可道」「名可名」谁都会撞上），四言以上
/// 还字字相等，才像是有人**对着**写的。
const PARALLEL_MIN_LEN: usize = 4;

// ── 输出 ──────────────────────────────────────────────────────────────────

/// **字词**一栏。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CharStats {
    /// 正文总字数（只数汉字，标点 / 空白 / 西文 / 夹注不进）。
    pub total: usize,
    /// 不重复字数。
    pub unique: usize,
    /// 多样性 = `unique / total`（0~1）。越低说明作者用词越集中。
    pub diversity: f32,
    /// 高频词组（已剪枝、已过滤虚词），按频次降序。
    pub top: Vec<(String, usize)>,
    /// 生僻字**去重**字数（常用字表之外）。
    pub rare: usize,
    /// 生僻字里属于**异体**的去重字数（⊆ `rare`）。
    pub variant: usize,
    /// 出现最多的几个生僻字（供 UI 摆成一小串「难字」），按频次降序。
    pub rare_top: Vec<(char, usize)>,
    /// **字汇覆盖率**（0~1）：最常用的 [`COVER_TOP`] 个字盖住了多少篇幅。
    ///
    /// 高 = 用字集中（叙事文、语录）；低 = 用字铺陈（辞赋、骈文）。
    /// 长文本上 `diversity` 必然趋近 0 而失去分辨力，这个数不会。
    pub cover: f32,
    /// **叠字**处数（AA 式：「悠悠」「蒼蒼」）。
    ///
    /// 《诗经》与骈散赋的招牌节奏——一望而知是「有韵的文字」，不是账本。
    /// 只数相邻同字，不判词性（「人人」也会被记下，那是诚实的代价）。
    pub redup: usize,
}

/// **句读**一栏。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct JoduStats {
    /// 「读」（，、；：—…）枚数。
    pub dou: usize,
    /// 「句」（。．！？…）枚数。
    pub ju: usize,
    /// 平均每句字数。零句时为 0（UI 应显示「—」而不是「0」）。
    pub avg: f32,
    /// 参与平均的句数（= `ju` + 末尾未收句的那一段，若有）。
    pub sentences: usize,
    /// **俪句**对数：相邻两句字数相等（且都 ≥ [`PARALLEL_MIN_LEN`] 字）。
    ///
    /// 骈文 / 赋的硬证据——「落霞与孤鹜齐飞，秋水共长天一色」这种是**对着
    /// 写**的，字字相等不是巧合。散文里也会撞上几对，故看的是**比例**而非
    /// 有无。
    pub parallel: usize,
    /// **四言句**的句数。
    ///
    /// 《老子》《论语》《诗经》的骨架就是四言——「道可道非常道」读起来
    /// 像格言，正是因为它卡在四字上。这个数高，文风几乎必是箴铭一类。
    pub four: usize,
}

/// **考据**一栏（校勘 · 注疏 · 训诂 · 名物）。
///
/// 这一栏是给「读书的人」而不是「看报表的人」准备的：文献学里真正费工夫的
/// 三件事——**注疏有多密、音义有多难、名物有多杂**——都能在这一屏里看出个
/// 大概。软件判不了真伪（那是人的活儿），但把**该去看一眼的地方**点出来，
/// 它做得比人快。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CritStats {
    /// **注疏**：夹注条数（原本注 + 用户注）。
    pub notes: usize,
    /// 注疏密度：**每百字**几条注。
    pub note_rate: f32,
    /// **注音**处数（书里自带的 `<ruby>` 与 `[拼音:]`）。
    pub ruby: usize,
    /// **通假**标注处数（含书内自定义通假表）。
    pub tongjia: usize,
    /// **名物**：人名处数。
    pub person: usize,
    /// 地名处数。
    pub place: usize,
    /// 书名处数。
    pub book: usize,
    /// **纪年**标记处数（干支「甲子」、数字年「三年」「元年」）。
    pub era: usize,
    /// **可疑**处数：三字重出（疑似衍文）与一字成句（疑似脱文）。
    ///
    /// ⚠️ 这是**提示，不是判决**。一字成句在文言里未必是错（「可。」「然。」），
    /// 三字重出也可能是作者有意（「徐徐」不算、「徐徐徐」才可疑）。它的价值
    /// 在于把「值得核一眼」的地方从十万字里捞出来——判定仍归人。
    pub odd: usize,
}

/// **文风**一栏。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StyleStats {
    /// 文言虚词密度：**每百字**出现几次。
    pub classical: f32,
    /// 白话标记密度：每百字出现几次。
    pub vernacular: f32,
    /// 书名号 / 专名号处数（引经据典的硬证据）。
    pub refs: usize,
    /// **记言密度**（每百字）：引语动词（[`SPEECH`]）出现几次。
    ///
    /// 高 = 语录 / 问答体（《论语》《孟子》《世说》），低 = 记事 / 论说
    /// （《通鉴》《韩非子》的论难部分）。**与「引经据典」是两回事**：引据
    /// 数的是「搬了谁的书」，记言数的是「记了谁的话」。
    pub speech: f32,
    /// 标签。
    pub label: &'static str,
}

impl StyleStats {
    /// 文白倾向（0~1）：1 = 纯文言，0 = 纯白话，0.5 = 分不出。
    ///
    /// UI 那根比例条直接吃这个值。两端都可能是 0（虚词一个没有），那时
    /// 返回 0.5 表示「无从判断」，别画成一根到底的条。
    pub fn bias(&self) -> f32 {
        let s = self.classical + self.vernacular;
        if s <= 0.0 {
            return 0.5;
        }
        (self.classical / s).clamp(0.0, 1.0)
    }
}

/// 一份完整分析结果。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TextStats {
    pub chars: CharStats,
    pub jodu: JoduStats,
    pub style: StyleStats,
    /// 考据（校勘 · 注疏 · 训诂 · 名物）。
    pub crit: CritStats,
}

impl TextStats {
    /// 一个字都没数到（空卡片 / 空选区）。UI 据此画空态，别画一堆 0。
    pub fn is_empty(&self) -> bool {
        self.chars.total == 0
    }
}

// ── 累加器 ────────────────────────────────────────────────────────────────

/// **逐段喂、最后一次算**的累加器。
///
/// 为什么要累加器而不是一个函数：本书 / 本章 / 选区 / 单卡四种口径共用
/// 同一套算法，且整本书是「一章一章从库里取」的（骨架模式），不可能一次
/// 拿到全文。累加器让壳子自己决定喂多少、何时停（配合 `mem::breathe`）。
#[derive(Clone, Debug, Default)]
pub struct Analyzer {
    total: usize,
    freq: HashMap<char, usize>,
    g2: HashMap<(char, char), usize>,
    g3: HashMap<(char, char, char), usize>,
    rare: HashMap<char, usize>,
    dou: usize,
    ju: usize,
    /// 上一枚「句」之后攒下的正文字数（末尾没收句的那半句也算一句）。
    pending: usize,
    classical: usize,
    vernacular: usize,
    /// 记言标记（曰 / 云 / 谓 / 问 / 对 / 答）命中数。
    speech_hits: usize,
    /// 书名号 / 专名号（`NoteKind::Book` / `Person` / `Place`）的处数。
    mark_spans: usize,
    /// 书名号 / 专名号覆盖的字数。
    mark_chars: usize,
    /// 正文里 `《` / `》` 的开闭枚数（无标注时用来兜底数「引了几处」）。
    q_latin: (usize, usize),
    /// 正文里 `「」` / `『』` 的开闭枚数。
    q_corner: (usize, usize),
    /// 每一句的字数（收句时记一笔，末尾那半句也算）。
    sent_lens: Vec<usize>,
    /// 叠字（AA 式）处数。
    redup: usize,
    /// 可疑处：三字重出 / 一字成句。
    odd: usize,
    /// 纪年标记处数。
    era: usize,
    // ── 注疏 / 训诂 / 名物 ──
    n_note: usize,
    n_ruby: usize,
    n_tongjia: usize,
    n_person: usize,
    n_place: usize,
    n_book: usize,
}

impl Analyzer {
    pub fn new() -> Self {
        Self::default()
    }

    /// 喂一段纯文本（无标注信息）。
    pub fn push(&mut self, text: &str) {
        self.push_with_marks(text, &[]);
    }

    /// 喂一段带标注的段落（章节里的 `Paragraph`）。
    ///
    /// `notes` 里只有**划线类标记**（人名 / 地名 / 书名）进统计：它们是
    /// 「引经据典」的硬证据（有人标过，比数《》靠谱）。夹注 / 注音不进——
    /// 注文不是作者的文风。
    pub fn push_with_marks(&mut self, text: &str, notes: &[Note]) {
        for n in notes {
            match n.kind {
                // 划线类：人名 / 地名 / 书名——「名物」的三种，分开记，
                // 因为「这部书引了多少书」和「这部书提了多少人」是两回事。
                NoteKind::Person => self.n_person += 1,
                NoteKind::Place => self.n_place += 1,
                NoteKind::Book => self.n_book += 1,
                // 注疏：原本夹注与用户自加的一并算（对读者而言都是「注」）。
                NoteKind::Note => self.n_note += 1,
                // 训诂：注音与通假——「音义有多难」的硬指标。
                NoteKind::Ruby => self.n_ruby += 1,
                NoteKind::Tongjia => self.n_tongjia += 1,
                // 着重号是**读者的**标记，不是文本的属性，不进统计。
                NoteKind::Emph => {}
            }
            if n.kind.is_mark() {
                self.mark_spans += 1;
                self.mark_chars += n.len.max(1);
            }
        }

        // 一行内按「汉字连续段」切开：标点 / 空白 / 西文都是断口。
        // n-gram 绝不能跨断口——「道，可」这种组合毫无意义。
        let mut run: Vec<char> = Vec::new();
        for c in text.chars() {
            if is_han(c) {
                self.total += 1;
                *self.freq.entry(c).or_default() += 1;
                if is_rare_char(c) {
                    *self.rare.entry(c).or_default() += 1;
                }
                if CLASSICAL.contains(&c) {
                    self.classical += 1;
                }
                if VERNACULAR.contains(&c) {
                    self.vernacular += 1;
                }
                if SPEECH.contains(&c) {
                    self.speech_hits += 1;
                }
                self.pending += 1;
                // 叠字（AA）：「悠悠」「蒼蒼」——《诗》与赋的招牌节奏。
                if run.last() == Some(&c) {
                    self.redup += 1;
                    // 三字重出多半是抄手串行（衍文），一字两见未必，故只记三连。
                    if run.len() >= 2 && run[run.len() - 2] == c {
                        self.odd += 1;
                    }
                }
                // 纪年：干支（甲子…）与数字年（三年 / 元年）。
                if let Some(&prev) = run.last() {
                    if c == '年' && NUM_HAN.contains(&prev) {
                        self.era += 1;
                    } else if GAN.contains(&prev) && ZHI.contains(&c) {
                        self.era += 1;
                    }
                }
                run.push(c);
                continue;
            }
            // 断口：先把攒下的连续段做成 n-gram，再判句读。
            self.flush_run(&run);
            run.clear();
            match c {
                '《' => self.q_latin.0 += 1,
                '》' => self.q_latin.1 += 1,
                '「' | '『' => self.q_corner.0 += 1,
                '」' | '』' => self.q_corner.1 += 1,
                _ => {}
            }
            match jodu_of(c, PunctMode::Classic) {
                Some(Jodu::Ring | Jodu::Dot) => {
                    self.ju += 1;
                    // 空句（连着两枚句号）不记——它不代表任何一句话。
                    if self.pending > 0 {
                        self.sent_lens.push(self.pending);
                    }
                    self.pending = 0;
                }
                Some(Jodu::Seed | Jodu::BigSeed) => self.dou += 1,
                None => {}
            }
        }
        self.flush_run(&run);
        // 末尾没收句的那半句也算一句（《老子》整本都这样收尾）。
        if self.pending > 0 {
            self.sent_lens.push(self.pending);
            self.pending = 0;
        }
    }

    /// 把一段连续的汉字做成 2 / 3 字 n-gram。
    fn flush_run(&mut self, run: &[char]) {
        let n = run.len();
        if n >= 2 {
            for w in run.windows(2) {
                if stop_hit(w) {
                    continue;
                }
                *self.g2.entry((w[0], w[1])).or_default() += 1;
            }
        }
        if n >= 3 {
            for w in run.windows(3) {
                if stop_hit(w) {
                    continue;
                }
                *self.g3.entry((w[0], w[1], w[2])).or_default() += 1;
            }
        }
    }

    /// 收工，出一份 [`TextStats`]。
    ///
    /// 除法全部有兜底：空文本不会 NaN，也不会 panic。
    pub fn finish(self) -> TextStats {
        let total = self.total;
        let unique = self.freq.len();
        let diversity = if total == 0 {
            0.0
        } else {
            (unique as f32 / total as f32).clamp(0.0, 1.0)
        };

        // ── 高频词组：剪枝 + 过滤 + 排序 ──
        let mut grams: Vec<(String, usize)> = Vec::new();
        for ((a, b, c), k) in self.g3.iter() {
            if *k < MIN_GRAM_HITS {
                continue;
            }
            // 冗余剪枝：3 字组的频次若与它包含的某个 2 字组**一样高**，
            // 说明多出来的那个字是白搭（「天下太」跟着「天下」一起出现），
            // 留下更短的那一形即可——标签云要的是「词」，不是「最长匹配」。
            let left = self.g2.get(&(*a, *b)).copied().unwrap_or(0);
            let right = self.g2.get(&(*b, *c)).copied().unwrap_or(0);
            if left == *k || right == *k {
                continue;
            }
            grams.push((format!("{a}{b}{c}"), *k));
        }
        for ((a, b), k) in self.g2.iter() {
            if *k >= MIN_GRAM_HITS {
                grams.push((format!("{a}{b}"), *k));
            }
        }
        // 同频次时：长的在前（「道德经」压过「道德」），再按字典序稳定排序。
        grams.sort_by(|x, y| {
            y.1.cmp(&x.1)
                .then_with(|| y.0.chars().count().cmp(&x.0.chars().count()))
                .then_with(|| x.0.cmp(&y.0))
        });
        grams.truncate(TOP_N);

        // ── 字汇覆盖率 ──
        // 长文本上 `diversity`（unique/total）必然趋近 0、失去分辨力，
        // 而「最常用的一百字盖住了多少篇幅」不会——它才是「用字集中与否」
        // 的真判据。
        let mut counts: Vec<usize> = self.freq.values().copied().collect();
        counts.sort_unstable_by(|a, b| b.cmp(a));
        let covered: usize = counts.iter().take(COVER_TOP).sum();
        let cover = if total == 0 {
            0.0
        } else {
            (covered as f32 / total as f32).clamp(0.0, 1.0)
        };

        // ── 生僻 / 异体 ──
        let mut rare_top: Vec<(char, usize)> = self.rare.iter().map(|(c, k)| (*c, *k)).collect();
        rare_top.sort_by(|x, y| y.1.cmp(&x.1).then_with(|| x.0.cmp(&y.0)));
        rare_top.truncate(TOP_N);
        let variant = self.rare.keys().copied().filter(|c| is_variant_char(*c)).count();

        // ── 句读 ──
        // 句长是逐句记下来的（`sent_lens`），故「平均」之外还能问两句更
        // 有意思的话：有几句是四言？有几对是字字相等的俪句？
        let sentences = self.sent_lens.len();
        let avg = if sentences == 0 || total == 0 {
            0.0
        } else {
            total as f32 / sentences as f32
        };
        let four = self.sent_lens.iter().filter(|n| **n == 4).count();
        // 俪句：相邻两句字数相等，且都够长（四言以上）——三言对三言太容易
        // 撞上，算不得「对着写」。
        let parallel = self
            .sent_lens
            .windows(2)
            .filter(|w| w[0] == w[1] && w[0] >= PARALLEL_MIN_LEN)
            .count();
        // 一字成句：可能是脱文，也可能只是「可。」——**只提示，不下判**。
        let odd = self.odd + self.sent_lens.iter().filter(|n| **n == 1).count();

        // ── 文风 ──
        let per100 = |hits: usize| -> f32 {
            if total == 0 {
                0.0
            } else {
                hits as f32 * 100.0 / total as f32
            }
        };
        let classical = per100(self.classical);
        let vernacular = per100(self.vernacular);
        let speech = per100(self.speech_hits);
        // 引经据典：优先信**标注**（有人标过的书名 / 专名，这是硬证据）；
        // 一处标注都没有才退回数《》「」的对子。**绝不两边都数**——同一个
        // 书名被标了又带括号，记两遍就是虚报。
        let quote_pairs = self.q_latin.0.min(self.q_latin.1) + self.q_corner.0.min(self.q_corner.1);
        let refs = if self.mark_spans > 0 {
            self.mark_spans
        } else {
            quote_pairs
        };
        let label = style_label(total, classical, vernacular, refs);

        TextStats {
            chars: CharStats {
                total,
                unique,
                diversity,
                top: grams,
                rare: self.rare.len(),
                variant,
                rare_top,
                cover,
                redup: self.redup,
            },
            jodu: JoduStats {
                dou: self.dou,
                ju: self.ju,
                avg,
                sentences,
                parallel,
                four,
            },
            style: StyleStats {
                classical,
                vernacular,
                refs,
                speech,
                label,
            },
            crit: CritStats {
                notes: self.n_note,
                note_rate: per100(self.n_note),
                ruby: self.n_ruby,
                tongjia: self.n_tongjia,
                person: self.n_person,
                place: self.n_place,
                book: self.n_book,
                era: self.era,
                odd,
            },
        }
    }

    /// 书名号 / 专名号覆盖的字数（UI 想显示「引文占比」时用得上）。
    pub fn mark_chars(&self) -> usize {
        self.mark_chars
    }
}

fn stop_hit(w: &[char]) -> bool {
    w.iter().any(|c| STOP.contains(c))
}

/// 文风标签的判据（抽成纯函数，好单测）。
///
/// 顺序即优先级：
///   1. 字数太少 → 不给标签（诚实优于好看）；
///   2. 有足够引文标注、且不是白话 → 引经据典；
///   3. 文言密度 ≥ 1.6 倍白话 → 偏文言；反之偏白话；
///   4. 都不成立 → 文白相杂（明清小说、近代整理本多半落这里）。
fn style_label(total: usize, classical: f32, vernacular: f32, refs: usize) -> &'static str {
    if total < MIN_LABEL_CHARS {
        return "字少难断";
    }
    if refs >= 3 && classical >= vernacular {
        return "引经据典";
    }
    if classical >= vernacular * LABEL_RATIO {
        return "偏文言";
    }
    if vernacular >= classical * LABEL_RATIO {
        return "偏白话";
    }
    "文白相杂"
}

/// **一句话分析**（选区 / 单张卡片 / 任何一段纯文本）。
pub fn analyze_text(text: &str) -> TextStats {
    let mut a = Analyzer::new();
    a.push(text);
    a.finish()
}

/// **一段段分析**（本章 / 本书）：壳子按自己的口径喂进来。
pub fn analyze_paras<'a>(paras: impl Iterator<Item = (&'a str, &'a [Note])>) -> TextStats {
    let mut a = Analyzer::new();
    for (t, notes) in paras {
        a.push_with_marks(t, notes);
    }
    a.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::NoteKind;

    fn note(at: usize, kind: NoteKind) -> Note {
        Note::new(at, kind, String::new())
    }

    #[test]
    fn empty_is_safe() {
        let s = analyze_text("");
        assert!(s.is_empty());
        assert_eq!(s.chars.diversity, 0.0);
        assert_eq!(s.jodu.avg, 0.0);
        assert_eq!(s.style.bias(), 0.5, "无虚词时不该画成一根到底的条");
        assert_eq!(s.style.label, "字少难断");
    }

    #[test]
    fn only_han_counts() {
        // 标点、空白、西文都不进「总字数」——《道德经》五千言别被算成六千。
        let s = analyze_text("道可道，非常道。 ABC 123");
        assert_eq!(s.chars.total, 6);
        assert_eq!(s.chars.unique, 4, "道/可/非/常");
    }

    #[test]
    fn jodu_split_and_avg() {
        let s = analyze_text("道可道，非常道。名可名，非常名。");
        assert_eq!(s.jodu.dou, 2, "两枚逗号");
        assert_eq!(s.jodu.ju, 2, "两枚句号");
        assert_eq!(s.jodu.sentences, 2);
        assert!((s.jodu.avg - 6.0).abs() < 0.01, "每句六字：{:?}", s.jodu);
    }

    #[test]
    fn trailing_fragment_counts_as_a_sentence() {
        // 末尾没收句的半句也算一句，否则平均句长虚高。
        let s = analyze_text("道可道，非常道。名可名");
        assert_eq!(s.jodu.ju, 1);
        assert_eq!(s.jodu.sentences, 2);
        assert!((s.jodu.avg - 4.5).abs() < 0.01, "{:?}", s.jodu);
    }

    #[test]
    fn classical_reads_as_classical() {
        let s = analyze_text(
            "子曰：學而時習之，不亦說乎？有朋自遠方來，不亦樂乎？人不知而不慍，不亦君子乎？",
        );
        assert_eq!(s.style.label, "偏文言", "{:?}", s.style);
        assert!(s.style.classical > s.style.vernacular);
    }

    #[test]
    fn modern_reads_as_vernacular() {
        let s = analyze_text(
            "他把那本书放在了桌子上，然后就走了，我也不知道他为什么要这样做，可能他很忙吧。",
        );
        assert_eq!(s.style.label, "偏白话", "{:?}", s.style);
    }

    #[test]
    fn refs_mark_it_as_allusive() {
        let mut a = Analyzer::new();
        a.push_with_marks(
            "君子之道，費而隱。夫婦之愚，可以與知焉，及其至也，雖聖人亦有所不知焉。夫婦之不肖，可以能行焉，及其至也，雖聖人亦有所不能焉。",
            &[
                note(0, NoteKind::Person),
                note(6, NoteKind::Book),
                note(20, NoteKind::Place),
            ],
        );
        let s = a.finish();
        assert_eq!(s.style.refs, 3);
        assert_eq!(s.style.label, "引经据典", "{:?}", s.style);
    }

    #[test]
    fn short_text_refuses_to_label() {
        // 三五个字判「偏文言」是胡说。
        let s = analyze_text("道可道，非常道。");
        assert_eq!(s.style.label, "字少难断");
        assert_eq!(s.chars.total, 6);
    }

    #[test]
    fn ngram_keeps_real_words_and_drops_particles() {
        // 「天下」反复出现；「之曰」这种虚词组合必须被滤掉。
        let s = analyze_text("天下有道，天下無道。天下有道，天下無道。");
        let words: Vec<&str> = s.chars.top.iter().map(|(w, _)| w.as_str()).collect();
        assert!(words.contains(&"天下"), "标签云里没有「天下」：{words:?}");
        assert!(words.contains(&"下有") || words.contains(&"無道") || words.contains(&"有道"));
        for w in words {
            assert!(
                !w.chars().any(|c| STOP.contains(&c)),
                "虚词组合漏进来了：{w}"
            );
        }
    }

    #[test]
    fn redundant_longer_gram_is_pruned() {
        // 「天下太」每一处都跟着「天下」一起出现（频次相同）→ 留短的那一形。
        let s = analyze_text("天下太平，天下太平，天下太平。");
        let words: Vec<&str> = s.chars.top.iter().map(|(w, _)| w.as_str()).collect();
        assert!(words.contains(&"天下"), "{words:?}");
        assert!(
            !words.contains(&"天下太"),
            "冗余三字组没剪掉：{words:?}"
        );
        assert!(
            words.iter().all(|w| w.chars().count() <= 3),
            "冒出四字组：{words:?}"
        );
    }

    #[test]
    fn quote_pairs_fallback_when_no_marks() {
        // 没人标书名时，退回数《》对子；成对才算，落单的半个不算。
        let s = analyze_text("讀《論語》與《孟子》，又讀《論語》一遍。");
        assert_eq!(s.style.refs, 3);
    }

    #[test]
    fn marks_win_over_quote_pairs() {
        // 标过就不再数括号：同一个书名绝不记两遍。
        let mut a = Analyzer::new();
        a.push_with_marks("讀《論語》，見仲尼。", &[note(1, NoteKind::Book)]);
        assert_eq!(a.finish().style.refs, 1);
    }

    #[test]
    fn rare_and_variant_are_nested() {
        // 埵：常用字表外（所以会被自动注音），且不是异体 → 只进 rare。
        assert!(is_rare_char('埵'));
        assert!(!is_variant_char('埵'));

        // U+F900 是 CJK **兼容**汉字区——Unicode 自己就写明它是异体。
        assert!(is_variant_char('\u{F900}'));

        let s = analyze_text("埵埵隰隰隰\u{F900}");
        assert_eq!(s.chars.rare, 3, "埵 / 隰 / U+F900");
        assert_eq!(s.chars.variant, 1, "只有兼容区那一枚是异体");
        assert!(s.chars.variant <= s.chars.rare, "异体必须是生僻的子集");
        assert_eq!(s.chars.rare_top[0], ('隰', 3));
    }

    #[test]
    fn common_chars_are_never_rare() {
        for c in ['道', '德', '經', '天', '下', '人'] {
            assert!(!is_rare_char(c), "{c} 被当生僻字了");
        }
    }

    #[test]
    fn han_set_excludes_kana_and_radicals() {
        assert!(is_han('道') && is_han('〇'));
        assert!(!is_han('あ'), "假名不是汉字");
        assert!(!is_han('\u{2F00}'), "康熙部首是符号，不是字");
        assert!(!is_han('，'));
    }

    #[test]
    fn diversity_is_a_ratio() {
        let s = analyze_text("道道道道");
        assert_eq!(s.chars.total, 4);
        assert_eq!(s.chars.unique, 1);
        assert!((s.chars.diversity - 0.25).abs() < 1e-6);
    }

    #[test]
    fn cover_rate_beats_diversity_on_long_text() {
        // 长文本上 diversity 必然趋近 0；覆盖率才是「用字集中与否」的真判据。
        let s = analyze_text("道可道非常道名可名非常名無名天地之始有名萬物之母");
        assert!(s.chars.cover > 0.5, "前百字盖不住一半：{:?}", s.chars);
        assert!(s.chars.cover <= 1.0);
    }

    #[test]
    fn redup_counts_doubled_chars() {
        // 「悠悠」「蒼蒼」两处叠字；三字重出另记进「可疑」。
        let s = analyze_text("悠悠蒼蒼，此何人哉？");
        assert_eq!(s.chars.redup, 2, "{:?}", s.chars);
    }

    #[test]
    fn triple_repeat_is_flagged_as_suspicious() {
        // 「徐徐徐」三连：多半是抄手串行（衍文），该提示人去核一眼。
        let s = analyze_text("其徐徐徐然，不可知也。");
        assert!(s.crit.odd >= 1, "三字重出没被记下：{:?}", s.crit);
    }

    #[test]
    fn parallel_and_four_character_lines() {
        // 两句都是六字且相邻 → 一对俪句；四言句另数。
        // 「落霞…」与「秋水…」六字相对 = 一对俪句；「關關雎鳩」四言一句。
        let s = analyze_text("落霞與孤鶩齊飛。秋水共長天一色。關關雎鳩。");
        assert_eq!(s.jodu.sentences, 3, "{:?}", s.jodu);
        assert_eq!(s.jodu.parallel, 1, "前两句六字相对：{:?}", s.jodu);
        assert_eq!(s.jodu.four, 1, "「關關雎鳩」是四言：{:?}", s.jodu);
    }

    #[test]
    fn speech_marks_quotation_style() {
        // 《论语》一路满篇「子曰」；《通鉴》记事则少得多。
        let s = analyze_text("子曰：學而時習之，不亦說乎？子曰：溫故而知新，可以為師矣。");
        assert!(s.style.speech > 2.0, "记言密度太低：{:?}", s.style);
    }

    #[test]
    fn crit_counts_notes_ruby_tongjia_and_names() {
        let mut a = Analyzer::new();
        a.push_with_marks(
            "莊子見惠子於濠梁之上，讀《南華真經》。",
            &[
                note(0, NoteKind::Note),
                note(0, NoteKind::Ruby),
                note(1, NoteKind::Tongjia),
                note(2, NoteKind::Person),
                note(6, NoteKind::Place),
                note(12, NoteKind::Book),
            ],
        );
        let c = a.finish().crit;
        assert_eq!(c.notes, 1);
        assert_eq!(c.ruby, 1);
        assert_eq!(c.tongjia, 1);
        assert_eq!((c.person, c.place, c.book), (1, 1, 1));
        assert_eq!(c.era, 0, "这句没有纪年：{c:?}");
    }

    #[test]
    fn era_marks_ganzhi_and_numbered_years() {
        // 干支「甲子」+ 数字年「三年」：史书的时间骨架。
        let s = analyze_text("甲子之年，王即位三年，諸侯來朝。");
        assert_eq!(s.crit.era, 2, "{:?}", s.crit);
    }

    #[test]
    fn paras_aggregate_without_averaging() {
        // 累加器必须**合并计数**，而不是把每段的结果平均——后者是错的。
        let paras: Vec<(&str, &[Note])> = vec![
            ("天下有道，卻走馬以糞。", &[]),
            ("天下無道，戎馬生於郊。", &[]),
        ];
        let s = analyze_paras(paras.into_iter());
        assert_eq!(s.chars.total, 18);
        assert_eq!(s.jodu.ju, 2);
    }
}
