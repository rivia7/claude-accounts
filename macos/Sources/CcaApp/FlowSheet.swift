import SwiftUI

/// 在 App 里问完的流程（提问会话，PROTOCOL 4.6）：cca 问一页，这里显示一页。问题、默认值、错误都是 cca 给的。
struct FlowSheet: View {
    @Environment(AppModel.self) private var model
    let flow: FlowModel

    var body: some View {
        Group {
            switch flow.phase {
            case .working:
                FlowWorking(flow: flow)
            case .asking(let e):
                if let c = e.confirmField {
                    FlowConfirm(flow: flow, ask: e, field: c).id(e.seq)
                } else {
                    FlowPage(flow: flow, ask: e).id(e.seq)
                }
            case .warned(let message, let warnings):
                FlowWarned(message: message, warnings: warnings) { model.closeFlow(flow) }
            case .failed(let f):
                FailureContent(title: "没能完成：\(flow.handoff.title)", failure: f, terminal: flow.handoff) {
                    model.closeFlow(flow)
                }
            }
        }
        .padding(22)
        .frame(width: 480)
        .interactiveDismissDisabled(true)
    }
}

private struct FlowHeader: View {
    let title: String
    let message: String

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(title).font(.title3.weight(.semibold))
            if !message.isEmpty {
                Text(message).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// 一页表单：一个或几个字段，“继续”交给 cca 校验，不合格的原因写在对应的输入框下面。
private struct FlowPage: View {
    let flow: FlowModel
    let ask: AskEvent
    @FocusState private var focus: String?

    /// 相邻的同一组放在一起（组名空着的不显示小标题）。
    private var sections: [(name: String, fields: [PromptField])] {
        var out: [(name: String, fields: [PromptField])] = []
        for f in ask.fields {
            if out.last?.name == f.section { out[out.count - 1].fields.append(f) } else { out.append((f.section, [f])) }
        }
        return out
    }

    private var fields: some View {
        VStack(alignment: .leading, spacing: 20) {
            ForEach(Array(sections.enumerated()), id: \.offset) { _, s in
                VStack(alignment: .leading, spacing: 12) {
                    if !s.name.isEmpty {
                        Text(s.name).font(.subheadline.weight(.semibold)).foregroundStyle(.secondary)
                    }
                    ForEach(s.fields) { f in
                        FlowField(flow: flow, field: f, focus: $focus)
                    }
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            FlowHeader(title: nonEmpty(ask.title) ?? flow.handoff.title, message: ask.message)
            if ask.fields.count > 6 {   // 字段多的表单（账号表）自己滚动，弹层不会撑得比屏幕还高
                ScrollView { fields.padding(.trailing, 14) }
                    .frame(height: 440)
            } else {
                fields
            }
            HStack {
                Spacer()
                Button("取消") { flow.cancel() }
                    .keyboardShortcut(.cancelAction)
                Button(nonEmpty(ask.submit) ?? "继续") { flow.submit() }
                    .keyboardShortcut(.defaultAction)
                    .buttonStyle(.glassProminent)
                    .disabled(!flow.canSubmit)
            }
        }
        .onAppear {
            let typed: (PromptField) -> Bool = { $0.kind == .text || $0.kind == .secret }
            let first = ask.fields.first { !$0.error.isEmpty && typed($0) } ?? ask.fields.first.flatMap { typed($0) ? $0 : nil }
            DispatchQueue.main.async { focus = first?.id }
        }
    }
}

private struct FlowField: View {
    let flow: FlowModel
    let field: PromptField
    var focus: FocusState<String?>.Binding

    var body: some View {
        let f = field
        VStack(alignment: .leading, spacing: 5) {
            if f.kind == .choice, f.style == "menu" {   // 一行：名字在左，弹出菜单在右
                HStack(spacing: 10) {
                    Text(f.label).lineLimit(1).truncationMode(.middle).help(f.label)
                    Spacer(minLength: 8)
                    FlowMenu(options: f.options, skip: f.optional ? nonEmpty(f.skip) ?? "不选" : nil,
                             selected: flow.draft[f.id] ?? "") { flow.draft[f.id] = $0 }
                }
            } else {
                if !f.label.isEmpty {
                    Text(f.label).font(.callout.weight(.medium))
                }
                control(f)
            }
            if !f.error.isEmpty {
                Label(f.error, systemImage: "exclamationmark.circle.fill")
                    .font(.caption)
                    .foregroundStyle(.red)
                    .fixedSize(horizontal: false, vertical: true)
                    .textSelection(.enabled)
            } else if !f.hint.isEmpty {
                Text(f.hint).font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    @ViewBuilder
    private func control(_ f: PromptField) -> some View {
        switch f.kind {
        case .secret:
            SecureField("", text: Binding(get: { flow.secret(f.id) }, set: { flow.setSecret(f.id, $0) }))
                .textFieldStyle(.roundedBorder)
                .focused(focus, equals: f.id)
                .onSubmit { flow.submit() }
        case .choice:
            FlowChoice(options: f.options, skip: f.optional ? nonEmpty(f.skip) ?? "先不选" : nil,
                       selected: flow.draft[f.id] ?? "") { flow.draft[f.id] = $0 }
        case .multi:
            FlowMulti(options: f.options, selected: flow.selected(f.id)) { flow.toggle(f.id, $0) }
        case .code:
            FlowCode(text: f.value)
        default:
            TextField("", text: Binding(get: { flow.draft[f.id] ?? "" }, set: { flow.draft[f.id] = $0 }))
                .textFieldStyle(.roundedBorder)
                .focused(focus, equals: f.id)
                .onSubmit { flow.submit() }
        }
    }
}

/// 选择题的一行：名字，后面淡淡地写注记（已被谁使用）。
private struct FlowOptionRow: View {
    let symbol: String
    let label: String
    let note: String
    let isOn: Bool
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Image(systemName: symbol)
                    .foregroundStyle(isOn ? AnyShapeStyle(.tint) : AnyShapeStyle(.secondary))
                Text(label)
                if !note.isEmpty {
                    Text(note).font(.caption).foregroundStyle(.secondary)
                }
                Spacer(minLength: 0)
            }
            .padding(.vertical, 3)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(isOn ? .isSelected : [])
    }
}

private struct FlowChoice: View {
    let options: [PromptOption]
    /// 可以不选时，“不选”那一项的说法（答案是空的）。
    let skip: String?
    let selected: String
    let pick: (String) -> Void

    var body: some View {
        FlowOptionList(count: options.count) {
            ForEach(options) { o in
                FlowOptionRow(symbol: selected == o.value ? "largecircle.fill.circle" : "circle", label: o.label, note: o.note,
                              isOn: selected == o.value) { pick(o.value) }
            }
            if let skip {
                FlowOptionRow(symbol: selected.isEmpty ? "largecircle.fill.circle" : "circle", label: skip, note: "",
                              isOn: selected.isEmpty) { pick("") }
            }
        }
    }
}

/// 一行里的弹出菜单（字段多的表单）。只有一个选项、没得选的（维护者固定了）只显示它。
private struct FlowMenu: View {
    let options: [PromptOption]
    /// 可以不选时，“不选”那一项的说法（答案是空的）。
    let skip: String?
    let selected: String
    let pick: (String) -> Void

    var body: some View {
        if options.count == 1, skip == nil, selected == options[0].value {
            Text(options[0].label).foregroundStyle(.secondary)
        } else {
            Menu(current) {
                if let skip { item(skip, value: "") }
                ForEach(options) { o in item(o.note.isEmpty ? o.label : "\(o.label)   \(o.note)", value: o.value) }
            }
            .fixedSize()
        }
    }

    /// 收起时只显示选中的名字（注记留在展开的菜单里），账号名不会被挤掉。
    private var current: String {
        options.first { $0.value == selected }?.label ?? (selected.isEmpty ? skip ?? "选择…" : selected)
    }

    private func item(_ text: String, value: String) -> some View {
        Button {
            pick(value)
        } label: {
            if selected == value { Label(text, systemImage: "checkmark") } else { Text(text) }
        }
    }
}

/// 只显示、不用回答的一段配置（等宽字体，可以复制）。
private struct FlowCode: View {
    @Environment(AppModel.self) private var model
    let text: String

    var body: some View {
        VStack(alignment: .trailing, spacing: 6) {
            ScrollView {
                Text(text)
                    .font(.system(.caption, design: .monospaced))
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(8)
            }
            .frame(height: 130)
            .background(RoundedRectangle(cornerRadius: 8).fill(.quaternary.opacity(0.6)))
            Button("复制") { model.copy(text) }
                .controlSize(.small)
        }
    }
}

private struct FlowMulti: View {
    let options: [PromptOption]
    let selected: Set<String>
    let toggle: (String) -> Void

    var body: some View {
        FlowOptionList(count: options.count) {
            ForEach(options) { o in
                FlowOptionRow(symbol: selected.contains(o.value) ? "checkmark.square.fill" : "square", label: o.label, note: o.note,
                              isOn: selected.contains(o.value)) { toggle(o.value) }
            }
        }
    }
}

/// 选项太多时列表自己滚动，弹层不会撑得比屏幕还高。
private struct FlowOptionList<Content: View>: View {
    let count: Int
    @ViewBuilder let content: Content

    var body: some View {
        if count > 8 {
            ScrollView {
                VStack(alignment: .leading, spacing: 2) { content }
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            .frame(maxHeight: 240)
        } else {
            VStack(alignment: .leading, spacing: 2) { content }
        }
    }
}

/// “是 / 否”的一页。默认“否”的（删除之类）回车不会同意。
private struct FlowConfirm: View {
    let flow: FlowModel
    let ask: AskEvent
    let field: PromptField

    var body: some View {
        let yesDefault = field.defaultValue.lowercased().hasPrefix("y")
        let warn = field.severity == .warn
        VStack(alignment: .leading, spacing: 14) {
            HStack(alignment: .top, spacing: 12) {
                if warn {
                    Image(systemName: "exclamationmark.triangle.fill")
                        .font(.system(size: 26))
                        .foregroundStyle(.orange)
                }
                VStack(alignment: .leading, spacing: 6) {
                    Text(nonEmpty(ask.title) ?? flow.handoff.title).font(.title3.weight(.semibold))
                    Text(field.label).fixedSize(horizontal: false, vertical: true)
                    if !ask.message.isEmpty {
                        Text(ask.message).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                    }
                    if !field.detail.isEmpty {
                        Text(field.detail).font(.callout).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                    }
                }
            }
            HStack {
                Spacer()
                if yesDefault {
                    Button("否") { flow.confirm(false) }
                        .keyboardShortcut(.cancelAction)
                    Button("是") { flow.confirm(true) }
                        .keyboardShortcut(.defaultAction)
                        .buttonStyle(.glassProminent)
                } else {
                    Button("是") { flow.confirm(true) }
                    Button("否") { flow.confirm(false) }
                        .keyboardShortcut(.defaultAction)
                        .buttonStyle(.glassProminent)
                }
            }
        }
        .onExitCommand { flow.confirm(false) }
    }
}

/// 等 cca：显示它最新的一行输出；等太久时可以停掉。
private struct FlowWorking: View {
    let flow: FlowModel

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            FlowHeader(title: flow.handoff.title, message: "")
            HStack(alignment: .firstTextBaseline, spacing: 10) {
                ProgressView().controlSize(.small)
                Text(nonEmpty(flow.output) ?? "正在处理…")
                    .foregroundStyle(.secondary)
                    .lineLimit(2)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            TimelineView(.periodic(from: .now, by: 5)) { ctx in
                if ctx.date.timeIntervalSince(flow.workingSince) > 30 {
                    HStack {
                        Text("这一步比平时久。").font(.callout).foregroundStyle(.secondary)
                        Spacer()
                        Button("停止") { flow.stop() }
                    }
                }
            }
        }
    }
}

/// 做完了，但有要提醒的。
private struct FlowWarned: View {
    let message: String
    let warnings: [String]
    let done: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack(alignment: .top, spacing: 12) {
                Image(systemName: "checkmark.circle.fill")
                    .font(.system(size: 26))
                    .foregroundStyle(.green)
                Text(message).font(.title3.weight(.semibold)).fixedSize(horizontal: false, vertical: true)
            }
            ForEach(warnings, id: \.self) { w in
                Label(w, systemImage: "exclamationmark.triangle")
                    .foregroundStyle(.orange)
                    .fixedSize(horizontal: false, vertical: true)
                    .textSelection(.enabled)
            }
            HStack {
                Spacer()
                Button("好", action: done)
                    .keyboardShortcut(.defaultAction)
                    .buttonStyle(.glassProminent)
            }
        }
    }
}
