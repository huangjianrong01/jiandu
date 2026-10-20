// 书架：书封墙 + 导入（多选文件）。

import SwiftUI
import UniformTypeIdentifiers

struct ShelfView: View {
    @EnvironmentObject var store: BookStore
    @State private var showImporter = false
    @State private var picked: [URL]?
    @State private var pendingDelete: ShelfBook?

    private let cols = [GridItem(.adaptive(minimum: 130), spacing: 18)]

    var body: some View {
        NavigationStack {
            ScrollView {
                LazyVGrid(columns: cols, spacing: 18) {
                    ForEach(store.books) { entry in
                        Button {
                            store.open(entry)
                        } label: {
                            BookCoverCard(entry: entry)
                        }
                        .buttonStyle(.plain)
                        .contextMenu {
                            Button(role: .destructive) {
                                pendingDelete = entry
                            } label: {
                                Label("从书架移除", systemImage: "trash")
                            }
                        }
                    }
                }
                .padding(18)
            }
            .background(Paper.paperBack)
            .navigationTitle("书架")
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button {
                        showImporter = true
                    } label: {
                        Label("导入书籍", systemImage: "plus.book")
                    }
                }
            }
            .sheet(isPresented: $showImporter) {
                DocumentPicker(urls: $picked, types: [.json, .plainText, .data])
            }
            .onChange(of: picked) { _, urls in
                guard let urls else { return }
                for u in urls { store.importFile(u) }
                picked = nil
            }
            .alert("从书架移除这本书？", isPresented: Binding(
                get: { pendingDelete != nil },
                set: { if !$0 { pendingDelete = nil } }
            ), presenting: pendingDelete) { entry in
                Button("移除", role: .destructive) { store.remove(entry) }
                Button("取消", role: .cancel) {}
            } message: { entry in
                Text("「\(entry.title)」的文件也会一并删除。")
            }
            .overlay {
                if store.books.isEmpty {
                    VStack(spacing: 10) {
                        Text("书架空空")
                            .font(.system(size: 16, weight: .medium))
                            .foregroundColor(Paper.ink)
                        Text("点右上角「导入书籍」（支持 json / md / txt / epub）")
                            .font(.system(size: 12))
                            .foregroundColor(Paper.frame)
                    }
                }
            }
        }
    }
}

/// 系统文件选择器（多选）。asCopy 会把书复制进 App 沙盒，避免安全作用域问题。
struct DocumentPicker: UIViewControllerRepresentable {
    @Binding var urls: [URL]?
    let types: [UTType]

    func makeUIViewController(context: Context) -> UIDocumentPickerViewController {
        let picker = UIDocumentPickerViewController(forOpeningContentTypes: types, asCopy: true)
        picker.allowsMultipleSelection = true
        picker.shouldShowFileExtensions = true
        return picker
    }

    func updateUIViewController(_ uiViewController: UIDocumentPickerViewController, context: Context) {}

    func makeCoordinator() -> Coordinator { Coordinator(self) }

    final class Coordinator: NSObject, UIDocumentPickerDelegate {
        let parent: DocumentPicker
        init(_ parent: DocumentPicker) { self.parent = parent }

        func documentPicker(_ controller: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL]) {
            parent.urls = urls
        }

        func documentPickerWasCancelled(_ controller: UIDocumentPickerViewController) {
            parent.urls = []
        }
    }
}
