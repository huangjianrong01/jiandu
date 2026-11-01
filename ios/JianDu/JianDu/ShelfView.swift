import SwiftUI
import UniformTypeIdentifiers
import UIKit

/// 朱砂红（与 Mac 端 / 笺校高亮同源），用于「内置」标签等点缀。
private let zhuSha = Color(red: 176 / 255, green: 52 / 255, blue: 44 / 255)

/// 书架首页：极简列表 + 原生 fileImporter 导入 PDF/PNG。
/// 点列表项 → 推进 ReaderView（阅读器）。右上角保留「排版探针」入口，
/// 直接复用已验证的 PoC 探针页（sheet 弹出），方便随时回看 Rust 引擎坐标。
struct ShelfView: View {
    @EnvironmentObject var store: BookStore
    @State private var showImporter = false
    @State private var showProbe = false

    var body: some View {
        NavigationStack {
            Group {
                if store.documents.isEmpty {
                    emptyState
                } else {
                    list
                }
            }
            .navigationTitle("笺读书架")
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Menu {
                        Button {
                            showImporter = true
                        } label: {
                            Label("导入 PDF / PNG", systemImage: "square.and.arrow.down")
                        }
                        Button {
                            showProbe = true
                        } label: {
                            Label("排版探针（调试）", systemImage: "ruler")
                        }
                    } label: {
                        Image(systemName: "ellipsis.circle")
                    }
                }
            }
            .navigationDestination(for: DocumentItem.self) { item in
                ReaderView(item: item)
            }
            .fileImporter(
                isPresented: $showImporter,
                allowedContentTypes: [.pdf, .png],
                allowsMultipleSelection: false
            ) { result in
                switch result {
                case .success(let urls):
                    if let url = urls.first {
                        // fileImporter 给的是安全作用域 URL，拷贝时仍需起始访问权限。
                        let ok = url.startAccessingSecurityScopedResource()
                        defer { if ok { url.stopAccessingSecurityScopedResource() } }
                        store.importFile(from: url)
                    }
                case .failure(let error):
                    store.lastError = error.localizedDescription
                }
            }
            .alert("导入失败", isPresented: Binding(
                get: { store.lastError != nil },
                set: { if !$0 { store.lastError = nil } }
            )) {
                Button("好", role: .cancel) { store.lastError = nil }
            } message: {
                Text(store.lastError ?? "")
            }
            .sheet(isPresented: $showProbe) {
                NavigationStack { ProbeView() }
            }
        }
    }

    private var list: some View {
        List {
            ForEach(store.documents) { doc in
                NavigationLink(value: doc) {
                    BookRow(item: doc)
                }
            }
            .onDelete { idx in
                idx.map { store.documents[$0] }.forEach { store.delete($0) }
            }
        }
        .listStyle(.insetGrouped)
    }

    private var emptyState: some View {
        VStack(spacing: 16) {
            Image(systemName: "books.vertical")
                .font(.system(size: 56))
                .foregroundStyle(.tertiary)
            Text("书架还是空的")
                .font(.title3)
                .foregroundStyle(.secondary)
            Text("点右上角「⋯」导入 PDF 或 PNG 开始")
                .font(.caption)
                .foregroundStyle(.tertiary)
        }
    }
}

/// 书架单行：内置书显示封面缩略图 + 朱砂「内置」标签 + 作者/简介；
/// 用户导入书沿用原系统图标 + 导入时间。
private struct BookRow: View {
    let item: DocumentItem
    @EnvironmentObject var store: BookStore

    var body: some View {
        HStack(spacing: 12) {
            if let cover = store.builtinCover(for: item) {
                Image(uiImage: cover)
                    .resizable()
                    .scaledToFill()
                    .frame(width: 46, height: 64)
                    .cornerRadius(4)
                    .clipped()
            } else {
                Image(systemName: item.kind.systemImage)
                    .font(.title2)
                    .foregroundStyle(.secondary)
                    .frame(width: 46, height: 64)
            }

            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 6) {
                    Text(item.name)
                        .font(.headline)
                    if item.isBuiltin {
                        Text("内置")
                            .font(.caption2)
                            .fontWeight(.medium)
                            .padding(.horizontal, 5)
                            .padding(.vertical, 1)
                            .background(zhuSha.opacity(0.12))
                            .foregroundStyle(zhuSha)
                            .cornerRadius(4)
                    }
                }
                if let author = store.builtinAuthor(for: item) {
                    Text(author)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                } else {
                    Text(item.addedAt.formatted(date: .abbreviated, time: .shortened))
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                if let intro = store.builtinIntro(for: item) {
                    Text(intro)
                        .font(.caption2)
                        .foregroundStyle(.tertiary)
                        .lineLimit(1)
                }
            }
            Spacer()
        }
        .padding(.vertical, 4)
    }
}
