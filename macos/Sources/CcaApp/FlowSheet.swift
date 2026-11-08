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

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            FlowHeader(title: nonEmpty(ask.title) ?? flow.handoff.title, message: ask.message)
            ForEach(ask.fields) { f in
                FlowField(flow: flow, field: f, focus: $focus)
            }
            HStack {
                Spacer()
                Button("取消") { flow.cancel() }
                    .keyboardShortcut(.cancelAction)
                Button("继续") { flow.submit() }
                    .keyboardShortcut(.defaultAction)
                    .buttonStyle(.glassProminent)
                    .disabled(!flow.canSubmit)
            }
        }
        .onAppear {
            let first = ask.fields.first { !$0.error.isEmpty && ($0.kind == .text || $0.kind == .secret) }
                ?? ask.fields.first { $0.kind == .text || $0.kind == .secret }
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
            if !f.label.isEmpty {
                Text(f.label).font(.callout.weight(.medium))
            }
            control(f)
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
