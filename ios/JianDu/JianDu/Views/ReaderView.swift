// 阅读视图：古文竖笺（逐字成列，从右到左）与现代横笺。
// 版式与 macOS 端一致：段首空两格（首行缩进）；繁体朱红小字、通假靛蓝小字、
// 注释朱红圈号（页脚列注文）。
//
// 本版新增：古文竖笺段首两字缩进、目录三级缩进、专注模式（隐藏工具栏/页脚/翻页）、
// 单页 / 双页（对开）铺满屏幕、点字查字典（Dict）。

import SwiftUI

/// 版面槽位（与 macOS 端的 flow / modern_slots 对应）。
private enum Slot {
    case indent              // 段首空两格
    case char(Character, chapter: Int, para: Int, offset: Int)
}

/// 由书构造槽位序列。
private func buildSlots(_ book: Book, script: Script, indent: Bool) -> [Slot] {
    var out: [Slot] = []
    out.reserveCapacity(book.totalChars + book.chapters.count * 3)
    for (ci, ch) in book.chapters.enumerated() {
        for (pi, p) in ch.paragraphs.enumerated() {
            if indent {
                out.append(.indent)
                out.append(.indent)
            }
            for (oi, c) in p.text.enumerated() {
                out.append(.char(c, chapter: ci, para: pi, offset: oi))
            }
        }
    }
    return out
}

/// 一页的版面参数（按可用空间自适应，铺满屏幕）。
private struct Layout: Equatable {
    var cell: CGFloat = 44
    var cols: Int = 8
    var rows: Int = 11
    var capacity: Int = 88
    var pages: Int = 1

    static func compute(script: Script, size: CGSize, slotCount: Int) -> Layout {
        let availW = max(1, size.width)
        let availH = max(1, size.height)
        var l = Layout()
        switch script {
        case .ancient:
            l.cols = Paper.ancientCols
            var cell = min(availW / CGFloat(l.cols), availH / 11.0)
            cell = max(20, min(cell, 56))
            var rows = Int(availH / cell)
            rows = max(8, min(rows, 16))
            cell = min(cell, availH / CGFloat(rows))
            l.cell = cell
            l.rows = rows
        case .modern:
            var cell = min(availW / 20.0, availH / 10.0)
            cell = max(12, min(cell, 30))
            var cols = Int(availW / cell)
            cols = max(14, min(cols, 26))
            var rows = Int(availH / cell)
            rows = max(6, min(rows, 14))
            cell = min(cell, availW / CGFloat(cols), availH / CGFloat(rows))
            l.cell = cell
            l.cols = cols
            l.rows = rows
        }
        l.capacity = l.cols * l.rows
        l.pages = max(1, Int((Double(slotCount) / Double(l.capacity)).rounded(.up)))
        return l
    }
}

struct ReaderView: View {
    @EnvironmentObject var store: BookStore
    @State private var page: Int = 0
    @State private var showTOC: Bool = {
        #if DEBUG
        return ProcessInfo.processInfo.environment["JIANDU_TOC"] == "1"
        #else
        return false
        #endif
    }()
    @State private var fontScale: CGFloat = 1.0
    @State private var showTraditional = true
    @State private var showTongjia = true
    @State private var showNotes = true
    @State private var indentFirstLine = true
    @State private var focusMode: Bool = {
        #if DEBUG
        return ProcessInfo.processInfo.environment["JIANDU_FOCUS"] == "1"
        #else
        return false
        #endif
    }()
    @State private var facing: Bool = {
        #if DEBUG
        return ProcessInfo.processInfo.environment["JIANDU_FACING"] == "1"
        #else
        return false
        #endif
    }()
    @State private var lookupChar: Character? = nil
    @State private var lastLayout = Layout()

    private var book: Book? { store.current }
    private var script: Script { store.currentScript }

    /// 全部槽位（随缩进开关变化）。
    private var slots: [Slot] {
        guard let book else { return [] }
        return buildSlots(book, script: script, indent: indentFirstLine)
    }

    private var leafCapacity: Int { max(1, lastLayout.capacity) }

    /// 某一叶（leaf）的槽位切片。
    private func slotsForLeaf(_ leaf: Int) -> [Slot] {
        let all = slots
        guard !all.isEmpty else { return [] }
        let start = min(leaf * leafCapacity, all.count)
        let end = min(start + leafCapacity, all.count)
        return Array(all[start..<end])
    }

    /// 当前可见的槽位（单页 = 一叶；双页 = 两叶）。
    private var visibleSlots: [Slot] {
        guard let _ = book else { return [] }
        if facing {
            return slotsForLeaf(page) + slotsForLeaf(page + 1)
        }
        return slotsForLeaf(page)
    }

    /// 注释编号表：(章, 段, 偏移) -> 全书顺序号。
    private var noteNumbers: [String: Int] {
        var map: [String: Int] = [:]
        var n = 0
        guard let book else { return map }
        for (ci, ch) in book.chapters.enumerated() {
            for (pi, p) in ch.paragraphs.enumerated() {
                for note in p.notes where note.kind == .note {
                    n += 1
                    map["\(ci):\(pi):\(note.at)"] = n
                }
            }
        }
        return map
    }

    /// 给定槽位集合中的页脚注释（按出现顺序）。
    private func notes(from slotsInPage: [Slot]) -> [(number: Int, text: String)] {
        var out: [(number: Int, text: String)] = []
        guard let book else { return out }
        for slot in slotsInPage {
            if case .char(_, let ci, let pi, let oi) = slot {
                if let n = noteNumbers["\(ci):\(pi):\(oi)"],
                   let note = book.chapters[ci].paragraphs[pi].note(at: oi) {
                    out.append((number: n, text: note.text))
                }
            }
        }
        return out
    }

    /// 当前可见槽位起始章（工具栏显示）。
    private var currentChapterTitle: String {
        guard let book else { return "" }
        for slot in visibleSlots {
            if case .char(_, let c, _, _) = slot { return c < book.chapters.count ? book.chapters[c].title : "" }
        }
        return ""
    }

    var body: some View {
        NavigationStack {
            VStack(spacing: 0) {
                if !focusMode { toolbarRow }
                GeometryReader { geo in
                    let half = facing ? max(1, geo.size.width / 2 - 6) : geo.size.width
                    let layout = Layout.compute(script: script, size: CGSize(width: half, height: geo.size.height), slotCount: slots.count)
                    ZStack {
                        PaperBackground().ignoresSafeArea()
                        if slots.isEmpty {
                            Text("这本书是空的")
                                .foregroundColor(Paper.frame)
                        } else if facing {
                            HStack(spacing: 12) {
                                leafView(slots: slotsForLeaf(page + 1), layout: layout, ancient: script == .ancient)
                                leafView(slots: slotsForLeaf(page), layout: layout, ancient: script == .ancient)
                            }
                        } else {
                            leafView(slots: slotsForLeaf(page), layout: layout, ancient: script == .ancient)
                        }
                    }
                    .onAppear { lastLayout = layout }
                    .onChange(of: layout) { _, v in lastLayout = v }
                }
                if !focusMode { footnotesView }
                if !focusMode { pagerRow }
            }
            .background(Paper.paper)
            .navigationBarHidden(true)
            .sheet(isPresented: $showTOC) { tocSheet }
            .sheet(isPresented: Binding(get: { lookupChar != nil }, set: { if !$0 { lookupChar = nil } })) {
                if let ch = lookupChar { DictionarySheet(character: ch, book: book) { lookupChar = nil } }
            }
            .onAppear {
                page = store.books.first(where: { $0.id == store.currentID })?.page ?? 0
                #if DEBUG
                // 仅调试构建：用环境变量直接弹出字典，便于无触摸注入时截图验证。
                if ProcessInfo.processInfo.environment["JIANDU_LOOKUP"] == "1" {
                    lookupChar = "道"
                }
                #endif
            }
            .onChange(of: page) { _, v in store.savePage(v) }
            .onChange(of: script) { _, v in
                page = 0
                store.setScript(v)
            }
            .onChange(of: facing) { _, _ in page = min(page, max(0, lastLayout.pages - 1)) }
            .onChange(of: indentFirstLine) { _, _ in page = min(page, max(0, lastLayout.pages - 1)) }
            .overlay(alignment: .topTrailing) {
                if focusMode {
                    Button {
                        withAnimation { focusMode = false }
                    } label: {
                        Label("退出专注", systemImage: "xmark.circle.fill")
                            .font(.system(size: 13, weight: .medium))
                            .foregroundColor(Paper.ink)
                            .padding(8)
                            .background(Paper.paperBack.opacity(0.85))
                            .clipShape(Capsule())
                    }
                    .padding(10)
                }
            }
            .simultaneousGesture(
                DragGesture(minimumDistance: 40).onEnded { v in
                    if v.translation.width > 40 {
                        goNext()
                    } else if v.translation.width < -40 {
                        goPrev()
                    }
                }
            )
        }
    }

    // MARK: - 工具栏

    private var toolbarRow: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 12) {
                Button { store.closeBook() } label: {
                    Image(systemName: "chevron.left").foregroundColor(Paper.ink)
                }
                Button { showTOC = true } label: {
                    Image(systemName: "list.bullet").foregroundColor(Paper.ink)
                }
                Picker("版式", selection: Binding(get: { script }, set: { store.setScript($0) })) {
                    Text("古文").tag(Script.ancient)
                    Text("横笺").tag(Script.modern)
                }
                .pickerStyle(.segmented)
                .frame(width: 130)

                Button { withAnimation { focusMode.toggle() } } label: {
                    Image(systemName: focusMode ? "eye.fill" : "eye").foregroundColor(Paper.ink)
                }
                Button { facing.toggle() } label: {
                    Image(systemName: facing ? "book.2.fill" : "book.2").foregroundColor(Paper.ink)
                }
                Button { indentFirstLine.toggle() } label: {
                    Image(systemName: indentFirstLine ? "paragraph.fill" : "paragraph").foregroundColor(Paper.ink)
                }
                Button { showTraditional.toggle() } label: {
                    Text("繁").font(.system(size: 13, weight: showTraditional ? .bold : .regular))
                        .foregroundColor(showTraditional ? Paper.cinnabar : Paper.frame)
                }
                Button { showTongjia.toggle() } label: {
                    Text("通").font(.system(size: 13, weight: showTongjia ? .bold : .regular))
                        .foregroundColor(showTongjia ? Paper.indigo : Paper.frame)
                }
                Button { showNotes.toggle() } label: {
                    Text("注").font(.system(size: 13, weight: showNotes ? .bold : .regular))
                        .foregroundColor(showNotes ? Paper.cinnabar : Paper.frame)
                }

                Spacer().frame(width: 8)

                Button { fontScale = max(0.7, fontScale - 0.1) } label: {
                    Text("A-").font(.system(size: 13, weight: .medium)).foregroundColor(Paper.ink)
                }
                Button { fontScale = min(1.6, fontScale + 0.1) } label: {
                    Text("A+").font(.system(size: 15, weight: .medium)).foregroundColor(Paper.ink)
                }
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
        }
        .background(Paper.paperBack)
    }

    // MARK: - 版面（可复用的单叶视图）

    @ViewBuilder
    private func leafView(slots leafSlots: [Slot], layout: Layout, ancient: Bool) -> some View {
        LeafView(
            slots: leafSlots,
            layout: layout,
            cell: layout.cell * fontScale,
            ancient: ancient,
            showTraditional: showTraditional,
            showTongjia: showTongjia,
            showNotes: showNotes,
            noteNumbers: noteNumbers,
            book: book,
            onCharTap: { lookupChar = $0 }
        )
    }

    // MARK: - 页脚注释

    @ViewBuilder
    private var footnotesView: some View {
        let items = notes(from: visibleSlots)
        if !items.isEmpty {
            ScrollView {
                VStack(alignment: .leading, spacing: 3) {
                    ForEach(Array(items.enumerated()), id: \.offset) { _, item in
                        Text("\(Trad.circled(item.number)) \(item.text)")
                            .font(.system(size: 11))
                            .foregroundColor(Paper.ink.opacity(0.85))
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                }
                .padding(.horizontal, 12)
            }
            .frame(maxHeight: 96)
            .background(Paper.paperBack.opacity(0.6))
        }
    }

    // MARK: - 翻页行

    private var pagerRow: some View {
        let total = lastLayout.pages
        let atEnd = facing ? (page + 1 >= total) : (page >= total - 1)
        return HStack {
            Button(action: goPrev) {
                Label("上一页", systemImage: "chevron.left")
                    .font(.system(size: 13))
                    .foregroundColor(page > 0 ? Paper.ink : Paper.frame.opacity(0.4))
            }
            .disabled(page <= 0)
            Spacer()
            Text("\(currentChapterTitle) · 第 \(page + 1) / \(total) 面")
                .font(.system(size: 12))
                .foregroundColor(Paper.frame)
            Spacer()
            Button(action: goNext) {
                Label("下一页", systemImage: "chevron.right")
                    .font(.system(size: 13))
                    .foregroundColor(atEnd ? Paper.frame.opacity(0.4) : Paper.ink)
            }
            .disabled(atEnd)
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 8)
        .background(Paper.paperBack)
    }

    // MARK: - 目录（三级缩进）

    private var tocSheet: some View {
        let toc = book?.toc ?? []
        let cur = chapterOfPage(page)
        let firstCur = toc.firstIndex(where: { $0.chapter == cur })
        return NavigationStack {
            List {
                ForEach(Array(toc.enumerated()), id: \.element.id) { idx, entry in
                    Button {
                        showTOC = false
                        jumpToChapter(entry.chapter)
                    } label: {
                        HStack {
                            if entry.level > 1 {
                                Spacer().frame(width: CGFloat((entry.level - 1) * 18))
                            }
                            Text(entry.title)
                                .font(.system(size: entry.level == 1 ? 16 : 15,
                                              weight: entry.level == 1 ? .semibold : .regular))
                                .foregroundColor(Paper.ink)
                            Spacer()
                            if idx == firstCur {
                                Image(systemName: "dot.square.fill")
                                    .foregroundColor(Paper.cinnabar)
                                    .font(.system(size: 10))
                            }
                        }
                    }
                }
            }
            .navigationTitle("目录")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button("完成") { showTOC = false }
                }
            }
        }
    }

    // MARK: - 动作

    private func goNext() {
        let limit = max(0, lastLayout.pages - 1)
        if page < limit { page += 1 }
    }

    private func goPrev() {
        if page > 0 { page -= 1 }
    }

    /// 某章第一面在全书中的页码。
    private func firstPage(ofChapter chapter: Int) -> Int {
        guard let book, leafCapacity > 0 else { return 0 }
        var count = 0
        for ci in 0..<min(chapter, book.chapters.count) {
            for p in book.chapters[ci].paragraphs {
                count += 1 + (indentFirstLine ? 2 : 0) + p.text.count
            }
        }
        return min(count / leafCapacity, max(0, lastLayout.pages - 1))
    }

    private func jumpToChapter(_ chapter: Int) {
        page = max(0, firstPage(ofChapter: chapter))
    }

    /// 当前页起始槽位属于哪一章。
    private func chapterOfPage(_ p: Int) -> Int {
        let leaf = slotsForLeaf(p)
        for slot in leaf {
            if case .char(_, let ci, _, _) = slot { return ci }
        }
        return 0
    }
}

/// 单叶（一个版面）的网格渲染。
private struct LeafView: View {
    let slots: [Slot]
    let layout: Layout
    let cell: CGFloat
    let ancient: Bool
    let showTraditional: Bool
    let showTongjia: Bool
    let showNotes: Bool
    let noteNumbers: [String: Int]
    let book: Book?
    let onCharTap: (Character) -> Void

    var body: some View {
        if ancient {
            let cols = columns(from: slots, rows: layout.rows)
            HStack(spacing: 0) {
                ForEach(Array(cols.enumerated()), id: \.offset) { _, col in
                    VStack(spacing: 0) {
                        ForEach(Array(col.enumerated()), id: \.offset) { _, slot in
                            cellView(slot)
                        }
                    }
                }
            }
            .environment(\.layoutDirection, .rightToLeft)
        } else {
            let rws = rows(from: slots, cols: layout.cols)
            VStack(spacing: 0) {
                ForEach(Array(rws.enumerated()), id: \.offset) { _, row in
                    HStack(spacing: 0) {
                        ForEach(Array(row.enumerated()), id: \.offset) { _, slot in
                            cellView(slot)
                        }
                    }
                }
            }
        }
    }

    private func columns(from slots: [Slot], rows: Int) -> [[Slot]] {
        guard rows > 0 else { return [] }
        var out: [[Slot]] = []
        var i = 0
        while i < slots.count {
            out.append(Array(slots[i..<min(i + rows, slots.count)]))
            i += rows
        }
        while out.count < Paper.ancientCols {
            out.append([])
        }
        return out
    }

    private func rows(from slots: [Slot], cols: Int) -> [[Slot]] {
        guard cols > 0 else { return [] }
        var out: [[Slot]] = []
        var i = 0
        while i < slots.count {
            out.append(Array(slots[i..<min(i + cols, slots.count)]))
            i += cols
        }
        return out
    }

    @ViewBuilder
    private func cellView(_ slot: Slot) -> some View {
        switch slot {
        case .indent:
            Color.clear
        case .char(let ch, let ci, let pi, let oi):
            ZStack {
                Text(String(ch))
                    .font(.system(size: cell * 0.78))
                    .foregroundColor(Paper.ink)

                if showTraditional, let t = Trad.traditional(ch) {
                    Text(String(t))
                        .font(.system(size: cell * 0.34))
                        .foregroundColor(Paper.cinnabar)
                        .offset(ancient ? CGSize(width: -cell * 0.52, height: 0)
                                        : CGSize(width: 0, height: -cell * 0.52))
                }

                if showTongjia, let t = Trad.tongjia(ch, bookTable: book?.tongjia ?? [:]) {
                    Text(String(t))
                        .font(.system(size: cell * 0.34))
                        .foregroundColor(Paper.indigo)
                        .offset(ancient ? CGSize(width: -cell * 0.52, height: cell * 0.32)
                                        : CGSize(width: 0, height: -cell * 0.30))
                }

                if showNotes, let n = noteNumbers["\(ci):\(pi):\(oi)"] {
                    Text(Trad.circled(n))
                        .font(.system(size: cell * 0.28))
                        .foregroundColor(Paper.cinnabar)
                        .offset(x: cell * 0.36, y: -cell * 0.34)
                }
            }
            .frame(width: cell, height: cell)
            .contentShape(Rectangle())
            .onTapGesture { onCharTap(ch) }
        }
    }
}

/// 点字查字典的底部弹窗。
private struct DictionarySheet: View {
    let character: Character
    let book: Book?
    let onDone: () -> Void

    private var def: String? { Dict.definition(of: character) }
    private var trad: Character? { Trad.traditional(character) }
    private var tj: Character? { Trad.tongjia(character, bookTable: book?.tongjia ?? [:]) }

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    HStack(alignment: .center, spacing: 14) {
                        Text(String(character))
                            .font(.system(size: 56, weight: .medium))
                            .foregroundColor(Paper.ink)
                        if let trad {
                            Text("繁：\(trad)")
                                .font(.system(size: 20))
                                .foregroundColor(Paper.cinnabar)
                        }
                        if let tj {
                            Text("通：本字「\(tj)」")
                                .font(.system(size: 16))
                                .foregroundColor(Paper.indigo)
                        }
                        Spacer()
                    }
                    if let def {
                        Text(def)
                            .font(.system(size: 16))
                            .foregroundColor(Paper.ink)
                            .frame(maxWidth: .infinity, alignment: .leading)
                    } else {
                        Text("字书暂未收录此字。")
                            .font(.system(size: 15))
                            .foregroundColor(Paper.frame)
                    }
                }
                .padding(20)
            }
            .navigationTitle("字义")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button("完成") { onDone() }
                }
            }
        }
    }
}
