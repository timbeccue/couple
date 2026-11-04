// Acknowledgements (docs/LICENSING.md §3): every bundled third-party component
// with its license name, and the license text as shipped in the bundle's
// Licenses folder (ffmpeg's COPYING.LGPLv2.1 / LICENSE.md / SOURCE.txt and
// the generated per-tree notices). Left: components grouped by tree; right:
// the notices file, scrolled to the selected component's section. Opened from
// the About panel (and the Help menu). Electron twin: views/AcknowledgementsView.tsx.

import SwiftUI

struct AcknowledgementsView: View {
    @State private var data = Acknowledgements.load()
    @State private var selectedComponent: String?
    @State private var documentID: String?

    var body: some View {
        Group {
            if data.isEmpty {
                missing
            } else {
                HSplitView {
                    componentList
                        .frame(minWidth: 320, idealWidth: 340, maxWidth: 440)
                    textPane
                        .frame(minWidth: 380)
                }
            }
        }
        .background(Theme.windowBg)
        .background(NonRestorableWindow())
        .onAppear {
            if selectedComponent == nil, let first = data.trees.first?.components.first {
                select(first.id)
            }
        }
    }

    // MARK: - left: components

    private var componentList: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text("Couple includes the following third-party software.")
                .font(.system(size: 11))
                .foregroundStyle(Theme.textSecondary)
                .padding(.horizontal, 12)
                .padding(.vertical, 10)
            List(selection: Binding(get: { selectedComponent }, set: { if let id = $0 { select(id) } })) {
                ForEach(data.trees) { tree in
                    Section {
                        if tree.components.isEmpty {
                            Text("None")
                                .font(.system(size: 11))
                                .foregroundStyle(Theme.textTertiary)
                        }
                        ForEach(tree.components) { c in
                            HStack(alignment: .firstTextBaseline, spacing: 6) {
                                Text(c.name).font(.system(size: 12))
                                Text(c.version)
                                    .font(.system(size: 10, design: .monospaced))
                                    .foregroundStyle(Theme.textTertiary)
                                Spacer(minLength: 8)
                                Text(c.usedUnder ?? c.license)
                                    .font(.system(size: 10))
                                    .foregroundStyle(Theme.textSecondary)
                                    .lineLimit(1)
                            }
                            .tag(c.id)
                        }
                    } header: {
                        Text(tree.components.isEmpty ? tree.title : "\(tree.title) (\(tree.components.count))")
                    }
                }
            }
            .listStyle(.sidebar)
        }
    }

    // MARK: - right: the notices file

    private var currentTree: NoticeTree? {
        guard let selectedComponent else { return nil }
        return data.trees.first { $0.documentByComponent[selectedComponent] != nil }
    }

    private var currentDocument: NoticeDocument? {
        guard let tree = currentTree else { return nil }
        if let documentID, let doc = tree.documents.first(where: { $0.id == documentID }) { return doc }
        return tree.documents.first
    }

    private var textPane: some View {
        VStack(alignment: .leading, spacing: 0) {
            if let tree = currentTree, tree.documents.count > 1 {
                Picker("File", selection: Binding(
                    get: { currentDocument?.id ?? "" },
                    set: { documentID = $0 })
                ) {
                    ForEach(tree.documents) { doc in
                        Text(doc.fileName).tag(doc.id)
                    }
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .padding(10)
            } else if let doc = currentDocument {
                Text(doc.fileName)
                    .font(.system(size: 11, design: .monospaced))
                    .foregroundStyle(Theme.textTertiary)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 10)
            }
            Divider()
            if let doc = currentDocument {
                ScrollViewReader { proxy in
                    ScrollView {
                        LazyVStack(alignment: .leading, spacing: 18) {
                            ForEach(doc.sections) { section in
                                sectionView(section)
                                    .id(section.id)
                            }
                        }
                        .padding(12)
                        .frame(maxWidth: .infinity, alignment: .leading)
                    }
                    .onChange(of: selectedComponent, initial: true) { _, id in
                        if let id { proxy.scrollTo(id, anchor: .top) }
                    }
                    .onChange(of: documentID) { _, _ in
                        if let id = selectedComponent { proxy.scrollTo(id, anchor: .top) }
                    }
                }
            }
        }
    }

    private func sectionView(_ section: NoticeSection) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(section.title)
                .font(.system(size: 12, weight: .semibold))
            ForEach(Array(section.header.enumerated()), id: \.offset) { _, line in
                Text(line)
                    .font(.system(size: 10.5, design: .monospaced))
                    .foregroundStyle(Theme.textSecondary)
            }
            Text(section.body)
                .font(.system(size: 10.5, design: .monospaced))
                .foregroundStyle(Theme.textPrimary)
                .textSelection(.enabled)
                .padding(.top, 4)
        }
    }

    // MARK: - empty

    private var missing: some View {
        VStack(spacing: 8) {
            Text("No license files in this build.")
                .font(.system(size: 13, weight: .medium))
            Text("Bundles made with Scripts/make-app.sh carry them in Contents/Resources/Licenses. Expected at:")
                .font(.system(size: 11))
                .foregroundStyle(Theme.textSecondary)
            Text(data.folder?.path ?? "(no bundle)")
                .font(.system(size: 10.5, design: .monospaced))
                .foregroundStyle(Theme.textTertiary)
                .textSelection(.enabled)
        }
        .padding(24)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private func select(_ id: String) {
        selectedComponent = id
        documentID = data.trees.first { $0.documentByComponent[id] != nil }?.documentByComponent[id]
    }
}
