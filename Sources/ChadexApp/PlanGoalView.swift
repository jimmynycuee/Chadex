import AppKit
import SwiftUI

/// The "Plan & Goal" operator panel. Approving a plan happens only here: the
/// model cannot approve its own plan.
struct PlanGoalView: View {
    @Environment(\.chadexLayout) private var layout
    @EnvironmentObject private var model: AppModel
    @StateObject private var panel = PlanGoalPanelModel()
    let project: ProjectRecord

    @State private var confirmation: PlanGoalConfirmation?
    @State private var rejectReason = ""
    @State private var skillExportNote: String?
    @State private var skillExportFailed = false

    var body: some View {
        VStack(spacing: 0) {
            emergencyBar
                .padding(.horizontal, layout.spacing(ChadexMetrics.detailHorizontalPadding))
                .padding(.top, layout.spacing(10))
                .padding(.bottom, layout.spacing(6))

            ChadexPageColumn {
                content
            }
        }
        .task(id: project.id) {
            guard model.selectedProject?.id == project.id else { return }
            panel.attach(model)
            panel.bind(projectID: project.id)
            rejectReason = ""
            while !Task.isCancelled {
                await panel.refresh()
                try? await Task.sleep(nanoseconds: UInt64(PlanGoalPanelModel.pollInterval * 1_000_000_000))
            }
        }
        .onChange(of: panel.selection) { _ in rejectReason = "" }
        .alert(item: $confirmation) { item in
            Alert(
                title: Text(item.title),
                message: Text(item.message),
                primaryButton: item.isDestructive
                    ? .destructive(Text(item.actionLabel)) { perform(item) }
                    : .default(Text(item.actionLabel)) { perform(item) },
                secondaryButton: .cancel(Text(L10n.string("common.cancel")))
            )
        }
    }

    // MARK: - Emergency stop

    @ViewBuilder
    private var emergencyBar: some View {
        if panel.emergencyStopEngaged {
            HStack(alignment: .center, spacing: layout.spacing(12)) {
                Image(systemName: "exclamationmark.octagon.fill")
                    .foregroundStyle(.red)
                    .accessibilityHidden(true)
                VStack(alignment: .leading, spacing: 2) {
                    Text(L10n.string("planGoal.emergency.engagedTitle"))
                        .chadexFont(.callout, weight: .semibold)
                    Text(L10n.string("planGoal.emergency.engagedMessage"))
                        .chadexFont(.caption)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
                Spacer(minLength: 12)
                Button(L10n.string("planGoal.emergency.release")) {
                    Task { await panel.releaseEmergencyStop() }
                }
                .buttonStyle(ChadexButtonStyle(kind: .secondary))
                .chadexControlSize(.small)
                .disabled(panel.emergencyInFlight)
            }
            .chadexAttentionCard(tint: .red, padding: 12)
            .accessibilityElement(children: .contain)
        } else {
            HStack(spacing: layout.spacing(10)) {
                Image(systemName: "shield.lefthalf.filled")
                    .foregroundStyle(.secondary)
                    .accessibilityHidden(true)
                Text(L10n.string("planGoal.emergency.hint"))
                    .chadexFont(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                Spacer(minLength: 12)
                Button(role: .destructive) {
                    confirmation = .emergencyStop
                } label: {
                    Label(L10n.string("planGoal.emergency.stop"), systemImage: "exclamationmark.octagon.fill")
                }
                .buttonStyle(ChadexButtonStyle(kind: .secondary))
                .chadexControlSize(.small)
                .disabled(panel.emergencyInFlight)
            }
        }
    }

    // MARK: - Content

    @ViewBuilder
    private var content: some View {
        ChadexPageHeader(
            title: L10n.string("planGoal.title"),
            subtitle: L10n.string("planGoal.subtitle")
        ) {
            if panel.loading { ProgressView().chadexControlSize(.small) }
            Button {
                Task { await panel.refresh() }
            } label: {
                Image(systemName: "arrow.clockwise")
            }
            .buttonStyle(.borderless)
            .help(L10n.string("planGoal.refresh"))
            .accessibilityLabel(L10n.string("planGoal.refresh"))
        }

        if let error = panel.overviewError {
            Label(error, systemImage: "exclamationmark.triangle")
                .chadexFont(.callout)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
                .textSelection(.enabled)
        }

        plansSection
        runsSection
        detailSection

        skillExportSection

        Label(L10n.string("planGoal.authorityNote"), systemImage: "lock.shield")
            .chadexFont(.caption)
            .foregroundStyle(.secondary)
            .fixedSize(horizontal: false, vertical: true)
    }

    // MARK: Skill export

    private var skillExportTunnelID: String {
        model.preferences.tunnelID.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private var canExportSkills: Bool {
        AppModel.isValidTunnelID(skillExportTunnelID) && SkillExport.bundledSkillsDirectory != nil
    }

    private var skillExportSection: some View {
        VStack(alignment: .leading, spacing: layout.spacing(8)) {
            SectionTitle(title: L10n.string("planGoal.skillExport.title"))
            Text(L10n.string("planGoal.skillExport.hint"))
                .chadexFont(.caption)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            HStack(spacing: layout.spacing(10)) {
                Button(L10n.string("planGoal.skillExport.button")) { exportSkills() }
                    .buttonStyle(ChadexButtonStyle(kind: .secondary))
                    .chadexControlSize(.small)
                    .disabled(!canExportSkills)
                if !canExportSkills {
                    Text(L10n.string("planGoal.skillExport.needTunnel"))
                        .chadexFont(.caption)
                        .foregroundStyle(.secondary)
                }
            }
            if let note = skillExportNote {
                Text(note)
                    .chadexFont(.caption)
                    .foregroundStyle(skillExportFailed ? Color.red : Color.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                    .textSelection(.enabled)
            }
        }
    }

    private func exportSkills() {
        guard let skills = SkillExport.bundledSkillsDirectory else { return }
        let panel = NSOpenPanel()
        panel.canChooseFiles = false
        panel.canChooseDirectories = true
        panel.canCreateDirectories = true
        panel.allowsMultipleSelection = false
        panel.prompt = L10n.string("planGoal.skillExport.choose")
        panel.directoryURL = FileManager.default.urls(for: .downloadsDirectory, in: .userDomainMask).first
        guard panel.runModal() == .OK, let folder = panel.url else { return }
        do {
            try SkillExport.export(tunnelID: skillExportTunnelID, skillsDirectory: skills, to: folder)
            skillExportFailed = false
            skillExportNote = L10n.string("planGoal.skillExport.done", folder.path)
        } catch {
            skillExportFailed = true
            skillExportNote = L10n.string("planGoal.skillExport.failed", error.localizedDescription)
        }
    }

    // MARK: Overview lists

    private var visiblePlans: [PlanSummary] {
        (panel.overview?.plans ?? []).filter { $0.state == "drafting" || $0.state == "approved" }
    }

    private var plansSection: some View {
        VStack(alignment: .leading, spacing: layout.spacing(8)) {
            SectionTitle(title: L10n.string("planGoal.plans.title"))
            if visiblePlans.isEmpty {
                emptyLabel(panel.overview == nil ? "planGoal.loading" : "planGoal.plans.none", "doc.text")
            } else {
                VStack(spacing: 0) {
                    ForEach(visiblePlans) { plan in
                        planRow(plan)
                        if plan.id != visiblePlans.last?.id {
                            Divider().padding(.leading, layout.spacing(12))
                        }
                    }
                }
                .chadexGroupSurface()
            }
        }
    }

    private func planRow(_ plan: PlanSummary) -> some View {
        Button {
            Task { await panel.toggle(.plan(plan.planId)) }
        } label: {
            HStack(alignment: .center, spacing: layout.spacing(10)) {
                VStack(alignment: .leading, spacing: 4) {
                    Text(plan.title)
                        .chadexFont(.callout, weight: .semibold)
                        .lineLimit(2)
                    HStack(spacing: 10) {
                        if plan.isGoalRequest && plan.state == "drafting" {
                            ChadexStatusLabel(
                                title: L10n.string("planGoal.goalRequest.waiting"),
                                systemImage: "flag.checkered.circle.fill",
                                tint: ChadexBrand.signal,
                                emphasized: true
                            )
                        } else {
                            planStateLabel(plan.state)
                        }
                        if plan.isPlanModeActive {
                            ChadexStatusLabel(
                                title: L10n.string("planGoal.planModeActive"),
                                systemImage: "lock.doc.fill",
                                tint: ChadexBrand.signal,
                                emphasized: true
                            )
                        }
                        Text(L10n.string("planGoal.revisionN", plan.latestRevision))
                            .chadexFont(.caption)
                            .foregroundStyle(.secondary)
                    }
                }
                Spacer(minLength: 8)
                Image(systemName: "chevron.right")
                    .foregroundStyle(.tertiary)
                    .accessibilityHidden(true)
            }
            .chadexPadding(12)
            .frame(maxWidth: .infinity, alignment: .leading)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .background(panel.selection == .plan(plan.planId) ? Color.accentColor.opacity(0.08) : Color.clear)
        .contextMenu {
            Button(L10n.string("planGoal.deletePlan"), systemImage: "trash", role: .destructive) {
                confirmation = .deletePlan(planId: plan.planId, title: plan.title)
            }
        }
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(.isButton)
    }

    private var runs: [GoalRun] {
        (panel.overview?.goalRuns ?? []).sorted { ($0.updatedAtUnixMs ?? 0) > ($1.updatedAtUnixMs ?? 0) }
    }

    /// Per project: runs still going plus the newest finished one. Older
    /// finished runs normally leave the lists in the backend already.
    private var shownRuns: [GoalRun] {
        var seenFinished = Set<String>()
        return runs.filter { run in
            guard run.isFinished else { return true }
            let project: String
            if case .string(let id) = run.project { project = id } else { project = "" }
            return seenFinished.insert(project).inserted
        }
    }

    private var runsSection: some View {
        VStack(alignment: .leading, spacing: layout.spacing(8)) {
            HStack(alignment: .firstTextBaseline) {
                SectionTitle(title: L10n.string("planGoal.runs.title"))
                Spacer(minLength: 8)
                if let overview = panel.overview, let running = overview.supervisorRunning {
                    ChadexStatusLabel(
                        title: L10n.string(running ? "planGoal.supervisor.running" : "planGoal.supervisor.stopped"),
                        systemImage: running ? "gearshape.2.fill" : "gearshape.2",
                        tint: running ? .green : .gray
                    )
                }
            }
            if shownRuns.isEmpty {
                emptyLabel(panel.overview == nil ? "planGoal.loading" : "planGoal.runs.none", "target")
            } else {
                VStack(spacing: 0) {
                    ForEach(shownRuns) { run in
                        runRow(run)
                        if run.id != shownRuns.last?.id {
                            Divider().padding(.leading, layout.spacing(12))
                        }
                    }
                }
                .chadexGroupSurface()
            }
        }
    }

    private func runRow(_ run: GoalRun) -> some View {
        Button {
            Task { await panel.toggle(.run(run.goalRunId)) }
        } label: {
            HStack(alignment: .center, spacing: layout.spacing(10)) {
                VStack(alignment: .leading, spacing: 4) {
                    HStack(spacing: 10) {
                        runStateLabel(run.state)
                        modeLabel(run)
                        Text(L10n.string("planGoal.iterationOf", run.iteration ?? 0, run.maxIterations ?? 0))
                            .chadexFont(.caption)
                            .foregroundStyle(.secondary)
                        Text(tokensText(run))
                            .chadexFont(.caption)
                            .foregroundStyle(.secondary)
                    }
                    chatgptHints(run)
                    if let reason = run.stateReason, !reason.isEmpty {
                        Text(reason)
                            .chadexFont(.caption)
                            .foregroundStyle(.secondary)
                            .lineLimit(2)
                    }
                    if let branch = run.resultBranch, !branch.isEmpty {
                        Label(branch, systemImage: "arrow.triangle.branch")
                            .chadexFont(.caption, design: .monospaced)
                            .foregroundStyle(.secondary)
                            .lineLimit(1)
                            .truncationMode(.middle)
                    }
                }
                Spacer(minLength: 8)
                Image(systemName: "chevron.right")
                    .foregroundStyle(.tertiary)
                    .accessibilityHidden(true)
            }
            .chadexPadding(12)
            .frame(maxWidth: .infinity, alignment: .leading)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .background(panel.selection == .run(run.goalRunId) ? Color.accentColor.opacity(0.08) : Color.clear)
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(.isButton)
    }

    private func tokensText(_ run: GoalRun) -> String {
        if run.cost == "tokens_reported_by_provider", let tokens = run.tokensUsed {
            return L10n.string("planGoal.tokens", tokens)
        }
        return L10n.string("planGoal.usageUnknown")
    }

    // MARK: Detail

    @ViewBuilder
    private var detailSection: some View {
        if let selection = panel.selection {
            VStack(alignment: .leading, spacing: layout.spacing(14)) {
                HStack {
                    SectionTitle(title: L10n.string(
                        { if case .plan = selection { return "planGoal.detail.plan" } else { return "planGoal.detail.run" } }()
                    ))
                    Spacer(minLength: 8)
                    Button(L10n.string("planGoal.detail.close")) {
                        Task { await panel.select(nil) }
                    }
                    .buttonStyle(ChadexButtonStyle(kind: .secondary))
                    .chadexControlSize(.small)
                }

                if let error = panel.actionError {
                    Label(error, systemImage: "exclamationmark.triangle.fill")
                        .chadexFont(.callout)
                        .foregroundStyle(.red)
                        .fixedSize(horizontal: false, vertical: true)
                        .textSelection(.enabled)
                }
                if let error = panel.detailError {
                    Label(error, systemImage: "exclamationmark.triangle")
                        .chadexFont(.callout)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                        .textSelection(.enabled)
                }

                switch selection {
                case .plan:
                    if let detail = panel.planDetail {
                        planDetailBody(detail)
                    } else if panel.detailError == nil {
                        ProgressView().chadexControlSize(.small)
                    }
                case .run:
                    if let detail = panel.runDetail {
                        runDetailBody(detail)
                    } else if panel.detailError == nil {
                        ProgressView().chadexControlSize(.small)
                    }
                }
            }
            .chadexCard(padding: 18)
        }
    }

    // MARK: Plan detail

    private func planDetailBody(_ detail: PlanInspectOutput) -> some View {
        let plan = detail.plan
        return VStack(alignment: .leading, spacing: layout.spacing(14)) {
            VStack(alignment: .leading, spacing: 6) {
                Text(plan.title)
                    .chadexFont(.headline, weight: .semibold)
                    .textSelection(.enabled)
                HStack(spacing: 10) {
                    planStateLabel(plan.state)
                    if plan.isPlanModeActive {
                        ChadexStatusLabel(
                            title: L10n.string("planGoal.planModeActive"),
                            systemImage: "lock.doc.fill",
                            tint: ChadexBrand.signal,
                            emphasized: true
                        )
                    }
                }
                Text(L10n.string("planGoal.projectLine", project.name))
                    .chadexFont(.caption)
                    .foregroundStyle(.secondary)
                if let revision = detail.revision {
                    Text(L10n.string("planGoal.revisionHash", revision.revision, revision.shortHash))
                        .chadexFont(.caption, design: .monospaced)
                        .foregroundStyle(.secondary)
                        .textSelection(.enabled)
                }
                if let reason = plan.closedReason, !reason.isEmpty {
                    Text(L10n.string("planGoal.closedReason", reason))
                        .chadexFont(.caption)
                        .foregroundStyle(.secondary)
                }
            }

            if let readiness = detail.gitReadiness, plan.state != "closed", plan.isGoalRequest {
                gitReadinessBlock(readiness)
            }

            if let revisions = detail.revisions, revisions.count > 1 {
                Picker(L10n.string("planGoal.revisionPicker"), selection: Binding(
                    get: { panel.viewedRevision ?? plan.latestRevision },
                    set: { value in Task { await panel.selectRevision(value == plan.latestRevision ? nil : value) } }
                )) {
                    ForEach(revisions.sorted { $0.revision > $1.revision }) { info in
                        Text(L10n.string("planGoal.revisionN", info.revision)).tag(info.revision)
                    }
                }
                .pickerStyle(.menu)
                .fixedSize()
            }

            if let revision = detail.revision {
                if let content = revision.content, !content.isEmpty {
                    detailBlock(title: "planGoal.content") {
                        Text(content)
                            .chadexFont(.callout)
                            .textSelection(.enabled)
                            .fixedSize(horizontal: false, vertical: true)
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                }
                acceptanceBlock(revision.spec)
                budgetBlock(revision.spec?.budget)
            }

            if let diff = detail.diff {
                diffBlock(diff)
            }

            decisionsBlock(detail.decisions ?? [])
            planActions(detail)
        }
    }

    @ViewBuilder
    private func gitReadinessBlock(_ readiness: GitReadiness) -> some View {
        if readiness.blocks {
            VStack(alignment: .leading, spacing: layout.spacing(8)) {
                Label(
                    L10n.string(readiness.isGitRepo == false ? "planGoal.git.notRepo" : "planGoal.git.noCommit"),
                    systemImage: "exclamationmark.triangle.fill"
                )
                .chadexFont(.callout, weight: .semibold)
                Text(L10n.string("planGoal.git.blockedExplain"))
                    .chadexFont(.caption)
                    .fixedSize(horizontal: false, vertical: true)
                DisclosureGroup(L10n.string("planGoal.git.howTo")) {
                    commitInstructions(readiness)
                }
                .chadexFont(.caption)
                Button {
                    Task { await panel.refresh() }
                } label: {
                    Label(L10n.string("planGoal.git.recheck"), systemImage: "arrow.clockwise")
                }
                .buttonStyle(ChadexButtonStyle(kind: .secondary))
                .chadexControlSize(.small)
                .disabled(panel.loading)
            }
            .chadexAttentionCard(tint: .orange, padding: 12)
        } else if readiness.hasUncommittedWork {
            VStack(alignment: .leading, spacing: 4) {
                Label(
                    L10n.string("planGoal.git.uncommitted", readiness.uncommittedFiles ?? 0),
                    systemImage: "info.circle"
                )
                .chadexFont(.caption)
                .foregroundStyle(.secondary)
                DisclosureGroup(L10n.string("planGoal.git.howTo")) {
                    commitInstructions(readiness)
                }
                .chadexFont(.caption)
                .foregroundStyle(.secondary)
            }
        }
    }

    private func commitInstructions(_ readiness: GitReadiness) -> some View {
        let commands = readiness.howToCommit ?? []
        return VStack(alignment: .leading, spacing: 6) {
            Text(L10n.string("planGoal.git.howToExplain"))
                .chadexFont(.caption)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            if !commands.isEmpty {
                Text(commands.joined(separator: "\n"))
                    .chadexFont(.caption, design: .monospaced)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .chadexPadding(8)
                    .background(Color.secondary.opacity(0.08), in: RoundedRectangle(cornerRadius: 6))
                Button {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(commands.joined(separator: "\n"), forType: .string)
                } label: {
                    Label(L10n.string("planGoal.git.copy"), systemImage: "doc.on.doc")
                }
                .buttonStyle(ChadexButtonStyle(kind: .secondary))
                .chadexControlSize(.small)
            }
            ForEach(readiness.tips ?? [], id: \.self) { tip in
                Label(tip, systemImage: "lightbulb")
                    .chadexFont(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(.top, 4)
    }

    private func acceptanceBlock(_ spec: PlanSpec?) -> some View {
        detailBlock(title: "planGoal.acceptance") {
            let checks = spec?.acceptance ?? []
            if checks.isEmpty {
                Text(L10n.string("planGoal.acceptance.none"))
                    .chadexFont(.caption)
                    .foregroundStyle(.secondary)
            } else {
                VStack(alignment: .leading, spacing: 8) {
                    ForEach(Array(checks.enumerated()), id: \.offset) { _, check in
                        VStack(alignment: .leading, spacing: 2) {
                            Text(check.name ?? "")
                                .chadexFont(.callout, weight: .medium)
                            Text(check.commandLine)
                                .chadexFont(.caption, design: .monospaced)
                                .textSelection(.enabled)
                            if let cwd = check.cwd, !cwd.isEmpty {
                                Text(L10n.string("planGoal.cwd", cwd))
                                    .chadexFont(.caption, design: .monospaced)
                                    .foregroundStyle(.secondary)
                            }
                        }
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
    }

    @ViewBuilder
    private func budgetBlock(_ budget: PlanBudget?) -> some View {
        if let budget {
            detailBlock(title: "planGoal.budget") {
                VStack(alignment: .leading, spacing: 2) {
                    if let value = budget.maxIterations {
                        Text(L10n.string("planGoal.budget.iterations", value))
                    }
                    if let value = budget.maxMinutes {
                        Text(L10n.string("planGoal.budget.minutes", value))
                    }
                    if let value = budget.agentTimeoutSecs {
                        Text(L10n.string("planGoal.budget.agentTimeout", value))
                    }
                }
                .chadexFont(.callout)
            }
        }
    }

    private func diffBlock(_ diff: PlanDiff) -> some View {
        detailBlock(title: "planGoal.diff") {
            VStack(alignment: .leading, spacing: 4) {
                Text(L10n.string("planGoal.diff.changed", diff.changedLines ?? 0))
                    .chadexFont(.caption)
                    .foregroundStyle(.secondary)
                let lines = diff.lines ?? []
                if !lines.isEmpty {
                    ScrollView(.horizontal) {
                        VStack(alignment: .leading, spacing: 0) {
                            ForEach(Array(lines.enumerated()), id: \.offset) { _, line in
                                Text(line.isEmpty ? " " : line)
                                    .chadexFont(.caption, design: .monospaced)
                                    .foregroundStyle(diffColor(line))
                                    .frame(maxWidth: .infinity, alignment: .leading)
                                    .background(diffBackground(line))
                            }
                        }
                        .textSelection(.enabled)
                    }
                }
                if diff.truncated == true {
                    Label(L10n.string("planGoal.diff.truncated"), systemImage: "ellipsis.circle")
                        .chadexFont(.caption)
                        .foregroundStyle(.secondary)
                }
            }
        }
    }

    private func diffColor(_ line: String) -> Color {
        line.hasPrefix("+") ? .green : (line.hasPrefix("-") ? .red : .primary)
    }

    private func diffBackground(_ line: String) -> Color {
        line.hasPrefix("+") ? Color.green.opacity(0.10) : (line.hasPrefix("-") ? Color.red.opacity(0.10) : .clear)
    }

    private func decisionsBlock(_ decisions: [PlanDecision]) -> some View {
        detailBlock(title: "planGoal.decisions") {
            if decisions.isEmpty {
                Text(L10n.string("planGoal.decisions.none"))
                    .chadexFont(.caption)
                    .foregroundStyle(.secondary)
            } else {
                VStack(alignment: .leading, spacing: 8) {
                    ForEach(decisions.sorted { ($0.createdAtUnixMs ?? 0) > ($1.createdAtUnixMs ?? 0) }) { decision in
                        VStack(alignment: .leading, spacing: 2) {
                            HStack(spacing: 10) {
                                decisionStateLabel(decision.state)
                                Text(L10n.string("planGoal.revisionHash", decision.revision, PlanGoalFormat.shortHash(decision.contentSha256)))
                                    .chadexFont(.caption, design: .monospaced)
                                    .foregroundStyle(.secondary)
                            }
                            let meta = [decision.approver, decision.channel, PlanGoalFormat.date(decision.createdAtUnixMs)]
                                .compactMap { $0 }.filter { !$0.isEmpty }.joined(separator: " · ")
                            if !meta.isEmpty {
                                Text(meta).chadexFont(.caption).foregroundStyle(.secondary)
                            }
                            if let reason = decision.reason, !reason.isEmpty {
                                Text(reason).chadexFont(.caption).textSelection(.enabled)
                            }
                            if let expires = PlanGoalFormat.date(decision.expiresAtUnixMs), decision.isActive {
                                Text(L10n.string("planGoal.expires", expires))
                                    .chadexFont(.caption).foregroundStyle(.secondary)
                            }
                            if let runId = decision.consumedByGoalRunId, !runId.isEmpty {
                                Text(L10n.string("planGoal.consumedBy", runId))
                                    .chadexFont(.caption, design: .monospaced).foregroundStyle(.secondary)
                            }
                            if decision.isActive {
                                Button(L10n.string("planGoal.revoke")) {
                                    confirmation = .revoke(approvalId: decision.approvalId, revision: decision.revision)
                                }
                                .buttonStyle(ChadexButtonStyle(kind: .secondary))
                                .chadexControlSize(.small)
                                .disabled(panel.actionInFlight)
                            }
                        }
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
    }

    /// One explicit way to start the Goal: a button plus a line saying what it costs.
    private func startOption(plan: PlanSummary, revision: PlanRevision, mode: String, disabled: Bool) -> some View {
        let suffix = mode == "chatgpt" ? "chatgpt" : "codex"
        return VStack(alignment: .leading, spacing: 4) {
            Button {
                confirmation = .approveAndStart(
                    planId: plan.planId, title: plan.title, isGoalRequest: plan.isGoalRequest,
                    revision: revision.revision, contentSha256: revision.contentSha256, mode: mode
                )
            } label: {
                Label(
                    plan.isGoalRequest
                        ? L10n.string("planGoal.start.\(suffix).goalRequest")
                        : L10n.string("planGoal.start.\(suffix)", revision.revision),
                    systemImage: mode == "chatgpt" ? "bubble.left.and.bubble.right" : "play.circle"
                )
            }
            .buttonStyle(ChadexButtonStyle(kind: mode == "chatgpt" ? .secondary : .primary))
            .disabled(disabled)
            Text(L10n.string("planGoal.start.\(suffix).note"))
                .chadexFont(.caption)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    @ViewBuilder
    private func chatgptHints(_ run: GoalRun) -> some View {
        if run.isChatgptIdle {
            Label(L10n.string("planGoal.chatgpt.idle"), systemImage: "moon.zzz")
                .chadexFont(.caption)
                .foregroundStyle(.orange)
        } else if run.isWaitingForChatgpt {
            Label(L10n.string("planGoal.chatgpt.waiting"), systemImage: "bubble.left.and.text.bubble.right")
                .chadexFont(.caption)
                .foregroundStyle(.orange)
        }
    }

    private func modeLabel(_ run: GoalRun) -> some View {
        Text(L10n.string(run.isChatgptMode ? "planGoal.mode.chatgpt" : "planGoal.mode.codex"))
            .chadexFont(.caption)
            .foregroundStyle(.secondary)
    }

    @ViewBuilder
    private func planActions(_ detail: PlanInspectOutput) -> some View {
        let plan = detail.plan
        if plan.state != "closed", let revision = detail.revision {
            let isLatest = revision.revision == plan.latestRevision
            let gitBlocked = detail.gitReadiness?.blocks ?? false
            let alreadyApproved = (detail.decisions ?? []).contains {
                $0.isActive && $0.revision == revision.revision && $0.contentSha256 == revision.contentSha256
            }
            VStack(alignment: .leading, spacing: layout.spacing(10)) {
                if !isLatest {
                    Label(L10n.string("planGoal.notLatest"), systemImage: "info.circle")
                        .chadexFont(.caption)
                        .foregroundStyle(.secondary)
                }
                if plan.isGoalRequest {
                    // A Goal starts from a commit; the user picks who does the work.
                    if gitBlocked {
                        Label(L10n.string("planGoal.git.buttonsDisabled"), systemImage: "lock")
                            .chadexFont(.caption)
                            .foregroundStyle(.secondary)
                    }
                    startOption(plan: plan, revision: revision, mode: "codex",
                                disabled: panel.actionInFlight || !isLatest || alreadyApproved || gitBlocked)
                    startOption(plan: plan, revision: revision, mode: "chatgpt",
                                disabled: panel.actionInFlight || !isLatest || alreadyApproved || gitBlocked)
                } else {
                    // A plan is implemented by ChatGPT itself once approved;
                    // it is not a Goal and uses no Codex quota.
                    VStack(alignment: .leading, spacing: 4) {
                        Button {
                            confirmation = .approve(
                                planId: plan.planId, title: plan.title,
                                revision: revision.revision, contentSha256: revision.contentSha256
                            )
                        } label: {
                            Label(
                                alreadyApproved
                                    ? L10n.string("planGoal.approved", revision.revision)
                                    : L10n.string("planGoal.approveForChatGPT", revision.revision),
                                systemImage: alreadyApproved ? "checkmark.seal.fill" : "checkmark.seal"
                            )
                        }
                        .buttonStyle(ChadexButtonStyle(kind: .primary))
                        .disabled(panel.actionInFlight || !isLatest || alreadyApproved)
                        Text(L10n.string("planGoal.approveForChatGPT.hint"))
                            .chadexFont(.caption)
                            .foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
                HStack(spacing: 10) {

                    if plan.isPlanModeActive {
                        Button {
                            confirmation = .closePlan(planId: plan.planId, title: plan.title)
                        } label: {
                            Label(L10n.string("planGoal.closePlan"), systemImage: "xmark.circle")
                        }
                        .buttonStyle(ChadexButtonStyle(kind: .secondary))
                        .disabled(panel.actionInFlight)
                    }

                    Button(role: .destructive) {
                        confirmation = .deletePlan(planId: plan.planId, title: plan.title)
                    } label: {
                        Label(L10n.string("planGoal.deletePlan"), systemImage: "trash")
                    }
                    .buttonStyle(ChadexButtonStyle(kind: .secondary))
                    .disabled(panel.actionInFlight)
                }
                if !plan.isGoalRequest {
                HStack(spacing: 10) {
                    TextField(L10n.string("planGoal.reject.placeholder"), text: $rejectReason)
                        .textFieldStyle(.roundedBorder)
                    Button {
                        let reason = rejectReason.trimmingCharacters(in: .whitespacesAndNewlines)
                        Task {
                            if await panel.requestChanges(
                                planId: plan.planId, revision: revision.revision,
                                contentSha256: revision.contentSha256, reason: reason
                            ) { rejectReason = "" }
                        }
                    } label: {
                        Label(L10n.string("planGoal.reject"), systemImage: "arrow.uturn.backward")
                    }
                    .buttonStyle(ChadexButtonStyle(kind: .secondary))
                    .disabled(panel.actionInFlight || !isLatest
                              || rejectReason.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                }
                }
            }
        }
    }

    // MARK: Run detail

    private func runDetailBody(_ detail: GoalStatusOutput) -> some View {
        let run = detail.goalRun
        return VStack(alignment: .leading, spacing: layout.spacing(14)) {
            VStack(alignment: .leading, spacing: 6) {
                runStateLabel(run.state)
                chatgptHints(run)
                if let reason = run.stateReason, !reason.isEmpty {
                    Text(reason).chadexFont(.callout).textSelection(.enabled)
                }
                if let request = run.controlRequest, !request.isEmpty {
                    Text(L10n.string("planGoal.controlRequest", request))
                        .chadexFont(.caption).foregroundStyle(.secondary)
                }
                Text(L10n.string("planGoal.iterationOf", run.iteration ?? 0, run.maxIterations ?? 0))
                    .chadexFont(.caption).foregroundStyle(.secondary)
                Text(L10n.string("planGoal.mode", L10n.string(run.isChatgptMode ? "planGoal.mode.chatgpt" : "planGoal.mode.codex")))
                    .chadexFont(.caption).foregroundStyle(.secondary)
                if let provider = run.provider, !provider.isEmpty {
                    Text(L10n.string("planGoal.provider", provider))
                        .chadexFont(.caption).foregroundStyle(.secondary)
                }
                Text(tokensText(run)).chadexFont(.caption).foregroundStyle(.secondary)
                if let branch = run.resultBranch, !branch.isEmpty {
                    Label(branch, systemImage: "arrow.triangle.branch")
                        .chadexFont(.caption, design: .monospaced)
                        .textSelection(.enabled)
                }
                if let path = run.workspacePath, !path.isEmpty {
                    Label(path, systemImage: "folder")
                        .chadexFont(.caption, design: .monospaced)
                        .foregroundStyle(.secondary)
                        .lineLimit(2)
                        .truncationMode(.middle)
                        .textSelection(.enabled)
                }
                if let revision = run.revision {
                    Text(L10n.string("planGoal.revisionHash", revision, PlanGoalFormat.shortHash(run.contentSha256)))
                        .chadexFont(.caption, design: .monospaced).foregroundStyle(.secondary)
                }
            }

            HStack(spacing: 10) {
                Button {
                    Task { await panel.pause(goalRunId: run.goalRunId) }
                } label: {
                    Label(L10n.string("planGoal.pause"), systemImage: "pause.circle")
                }
                .buttonStyle(ChadexButtonStyle(kind: .secondary))
                .disabled(panel.actionInFlight || !run.canPause)

                Button {
                    Task { await panel.resume(goalRunId: run.goalRunId) }
                } label: {
                    Label(L10n.string("planGoal.resume"), systemImage: "play.circle")
                }
                .buttonStyle(ChadexButtonStyle(kind: .secondary))
                .disabled(panel.actionInFlight || !run.canResume)

                Button(role: .destructive) {
                    confirmation = .cancelRun(goalRunId: run.goalRunId)
                } label: {
                    Label(L10n.string("planGoal.cancel"), systemImage: "xmark.circle")
                }
                .buttonStyle(ChadexButtonStyle(kind: .secondary))
                .disabled(panel.actionInFlight || !run.canCancel)

                Button(role: .destructive) {
                    confirmation = .deleteRun(goalRunId: run.goalRunId)
                } label: {
                    Label(L10n.string("planGoal.deleteRun"), systemImage: "trash")
                }
                .buttonStyle(ChadexButtonStyle(kind: .secondary))
                .disabled(panel.actionInFlight || !run.isFinished)
                .help(run.isFinished ? "" : L10n.string("planGoal.deleteRun.unfinished"))
            }

            detailBlock(title: "planGoal.events") {
                let events = (detail.events ?? []).sorted { $0.seq > $1.seq }
                if events.isEmpty {
                    Text(L10n.string("planGoal.events.none"))
                        .chadexFont(.caption).foregroundStyle(.secondary)
                } else {
                    VStack(alignment: .leading, spacing: 6) {
                        ForEach(events) { event in
                            VStack(alignment: .leading, spacing: 1) {
                                HStack(spacing: 8) {
                                    Text(event.kind).chadexFont(.caption, weight: .semibold)
                                    if let when = PlanGoalFormat.date(event.atUnixMs) {
                                        Text(when).chadexFont(.caption).foregroundStyle(.secondary)
                                    }
                                }
                                let detailText = PlanGoalFormat.compact(event.detail)
                                if !detailText.isEmpty {
                                    Text(detailText)
                                        .chadexFont(.caption, design: .monospaced)
                                        .foregroundStyle(.secondary)
                                        .lineLimit(3)
                                        .textSelection(.enabled)
                                }
                            }
                        }
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
            }

            detailBlock(title: "planGoal.operations") {
                let operations = detail.operations ?? []
                if operations.isEmpty {
                    Text(L10n.string("planGoal.operations.none"))
                        .chadexFont(.caption).foregroundStyle(.secondary)
                } else {
                    VStack(alignment: .leading, spacing: 6) {
                        ForEach(operations) { operation in
                            HStack(spacing: 8) {
                                Text(operation.kind ?? operation.opKey)
                                    .chadexFont(.caption, weight: .semibold)
                                if let iteration = operation.iteration {
                                    Text("#\(iteration)").chadexFont(.caption).foregroundStyle(.secondary)
                                }
                                if let state = operation.state {
                                    Text(state).chadexFont(.caption).foregroundStyle(.secondary)
                                }
                            }
                        }
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
            }
        }
    }

    // MARK: Shared pieces

    private func detailBlock<Body: View>(title: String, @ViewBuilder body: () -> Body) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(L10n.string(title))
                .chadexFont(.caption, weight: .semibold)
                .foregroundStyle(.secondary)
            body()
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private func emptyLabel(_ key: String, _ symbol: String) -> some View {
        Label(L10n.string(key), systemImage: symbol)
            .chadexFont(.callout)
            .foregroundStyle(.secondary)
            .chadexPadding(.vertical, 6)
    }

    private func stateText(_ prefix: String, _ state: String) -> String {
        let key = "planGoal.\(prefix).\(state)"
        let text = L10n.string(key)
        return text == key ? state : text
    }

    private func planStateLabel(_ state: String) -> some View {
        let (symbol, tint): (String, Color) = {
            switch state {
            case "drafting": return ("pencil.circle.fill", .orange)
            case "approved": return ("checkmark.seal.fill", .green)
            default: return ("xmark.circle.fill", .gray)
            }
        }()
        return ChadexStatusLabel(title: stateText("planState", state), systemImage: symbol, tint: tint, emphasized: true)
    }

    private func runStateLabel(_ state: String) -> some View {
        let (symbol, tint): (String, Color) = {
            switch state {
            case "queued": return ("clock.fill", .gray)
            case "running": return ("play.circle.fill", ChadexBrand.signal)
            case "paused": return ("pause.circle.fill", .orange)
            case "needs_attention": return ("exclamationmark.triangle.fill", .orange)
            case "succeeded": return ("checkmark.circle.fill", .green)
            case "failed": return ("xmark.octagon.fill", .red)
            case "cancelled": return ("slash.circle.fill", .gray)
            case "stopped": return ("stop.circle.fill", .red)
            default: return ("questionmark.circle.fill", .gray)
            }
        }()
        return ChadexStatusLabel(title: stateText("runState", state), systemImage: symbol, tint: tint, emphasized: true)
    }

    private func decisionStateLabel(_ state: String) -> some View {
        let (symbol, tint): (String, Color) = {
            switch state {
            case "active": return ("checkmark.seal.fill", .green)
            case "rejected": return ("arrow.uturn.backward.circle.fill", .orange)
            case "consumed": return ("target", ChadexBrand.signal)
            case "revoked": return ("xmark.seal.fill", .red)
            case "superseded": return ("arrow.triangle.2.circlepath", .gray)
            case "expired": return ("clock.badge.exclamationmark.fill", .gray)
            default: return ("questionmark.circle.fill", .gray)
            }
        }()
        return ChadexStatusLabel(title: stateText("decisionState", state), systemImage: symbol, tint: tint, emphasized: true)
    }

    // MARK: Confirmed actions

    private func perform(_ item: PlanGoalConfirmation) {
        switch item {
        case .approve(let planId, _, let revision, let sha):
            // Exactly the revision and hash shown in the confirmation.
            Task { await panel.approve(planId: planId, revision: revision, contentSha256: sha) }
        case .approveAndStart(let planId, _, _, let revision, let sha, let mode):
            Task { await panel.approveAndStart(planId: planId, revision: revision, contentSha256: sha, mode: mode) }
        case .closePlan(let planId, _):
            Task { await panel.closePlan(planId: planId) }
        case .revoke(let approvalId, _):
            Task { await panel.revoke(approvalId: approvalId) }
        case .cancelRun(let goalRunId):
            Task { await panel.cancel(goalRunId: goalRunId) }
        case .deletePlan(let planId, _):
            Task { await panel.deletePlan(planId: planId) }
        case .deleteRun(let goalRunId):
            Task { await panel.deleteRun(goalRunId: goalRunId) }
        case .emergencyStop:
            Task { await panel.emergencyStop(reason: nil) }
        }
    }
}

/// A destructive or approving action waiting for the person's confirmation.
/// It captures the displayed values so the request matches the dialog.
enum PlanGoalConfirmation: Identifiable {
    case approve(planId: String, title: String, revision: Int, contentSha256: String)
    case approveAndStart(planId: String, title: String, isGoalRequest: Bool, revision: Int, contentSha256: String, mode: String)
    case closePlan(planId: String, title: String)
    case revoke(approvalId: String, revision: Int)
    case cancelRun(goalRunId: String)
    case deletePlan(planId: String, title: String)
    case deleteRun(goalRunId: String)
    case emergencyStop

    var id: String {
        switch self {
        case .approve(let planId, _, let revision, let sha): return "approve:\(planId):\(revision):\(sha)"
        case .approveAndStart(let planId, _, _, let revision, let sha, let mode): return "start:\(planId):\(revision):\(sha):\(mode)"
        case .closePlan(let planId, _): return "close:\(planId)"
        case .revoke(let approvalId, _): return "revoke:\(approvalId)"
        case .cancelRun(let id): return "cancel:\(id)"
        case .deletePlan(let planId, _): return "deletePlan:\(planId)"
        case .deleteRun(let id): return "deleteRun:\(id)"
        case .emergencyStop: return "emergency"
        }
    }

    var title: String {
        switch self {
        case .approve(_, _, let revision, _): return L10n.string("planGoal.approve.confirmTitle", revision)
        case .approveAndStart(_, _, let isGoalRequest, let revision, _, let mode):
            let suffix = mode == "chatgpt" ? "chatgpt" : "codex"
            return isGoalRequest
                ? L10n.string("planGoal.start.\(suffix).goalRequest.confirmTitle")
                : L10n.string("planGoal.start.\(suffix).confirmTitle", revision)
        case .closePlan: return L10n.string("planGoal.closePlan.confirmTitle")
        case .revoke: return L10n.string("planGoal.revoke.confirmTitle")
        case .cancelRun: return L10n.string("planGoal.cancel.confirmTitle")
        case .deletePlan: return L10n.string("planGoal.deletePlan.confirmTitle")
        case .deleteRun: return L10n.string("planGoal.deleteRun.confirmTitle")
        case .emergencyStop: return L10n.string("planGoal.emergency.confirmTitle")
        }
    }

    var message: String {
        switch self {
        case .approve(_, let title, let revision, let sha):
            return L10n.string("planGoal.approve.confirmMessage", title, revision, PlanGoalFormat.shortHash(sha))
        case .approveAndStart(_, let title, _, let revision, let sha, let mode):
            let suffix = mode == "chatgpt" ? "chatgpt" : "codex"
            return L10n.string("planGoal.start.\(suffix).confirmMessage", title, revision, PlanGoalFormat.shortHash(sha))
        case .closePlan(_, let title): return L10n.string("planGoal.closePlan.confirmMessage", title)
        case .revoke(_, let revision): return L10n.string("planGoal.revoke.confirmMessage", revision)
        case .cancelRun: return L10n.string("planGoal.cancel.confirmMessage")
        case .deletePlan(_, let title): return L10n.string("planGoal.deletePlan.confirmMessage", title)
        case .deleteRun: return L10n.string("planGoal.deleteRun.confirmMessage")
        case .emergencyStop: return L10n.string("planGoal.emergency.confirmMessage")
        }
    }

    var actionLabel: String {
        switch self {
        case .approve(_, _, let revision, _): return L10n.string("planGoal.approve", revision)
        case .approveAndStart(_, _, let isGoalRequest, let revision, _, let mode):
            let suffix = mode == "chatgpt" ? "chatgpt" : "codex"
            return isGoalRequest
                ? L10n.string("planGoal.start.\(suffix).goalRequest")
                : L10n.string("planGoal.start.\(suffix)", revision)
        case .closePlan: return L10n.string("planGoal.closePlan")
        case .revoke: return L10n.string("planGoal.revoke")
        case .cancelRun: return L10n.string("planGoal.cancel")
        case .deletePlan: return L10n.string("planGoal.deletePlan")
        case .deleteRun: return L10n.string("planGoal.deleteRun")
        case .emergencyStop: return L10n.string("planGoal.emergency.stop")
        }
    }

    var isDestructive: Bool {
        if case .approve = self { return false }
        if case .approveAndStart = self { return false }
        return true
    }
}
