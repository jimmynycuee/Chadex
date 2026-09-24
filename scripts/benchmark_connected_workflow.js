// Run inside functions.exec, whose tools namespace supplies the two connectors.
// Each pair runs sequentially. Never parallelize the five dependent steps.
// Fixture reset/evaluator are outside timing and touch only the two test files.
async function connectedRun(provider, projects) {
    const project = projects?.[provider];
    if (!project) throw Error(`missing explicit project id for ${provider}`);
    const samples = [];
    async function call(name, args) {
        const start = Date.now();
        const result = await tools[`mcp__codex_apps__${provider}_${name}`]({ project, ...args });
        const ms = Date.now() - start;
        if (result.isError || result.structuredContent?.success !== true) {
            throw Error(`${provider}/${name} failed: ${JSON.stringify(result).slice(0, 1200)}`);
        }
        const output = result.structuredContent.output;
        if (output.items?.some(item => item.success === false)) throw Error(`${name} item failed`);
        samples.push({tool: name, started_at_ms: start, ms});
        return output;
    }
    await call('search_project_texts', { queries: [
        { path: 'pricing/discount.py', pattern: 'return round(amount - discount_amount, 2)', pattern_mode: 'literal', limit: 5 },
        { path: 'tests/test_checkout.py', pattern: 'def test_zero_discount', pattern_mode: 'literal', limit: 5 },
    ], max_result_bytes: 8192 });
    const read = await call('read_files', { items: [
        {path: 'pricing/discount.py'}, {path: 'tests/test_checkout.py'},
    ], max_result_bytes: 32768 });
    if (!read.items[0].output.text.includes('return round(amount - discount_amount, 2)')) throw Error('wrong baseline');
    const changes = [
        {kind: 'edit', path: 'pricing/discount.py', edits: [{kind: 'replace_exact', old_text: 'return round(amount - discount_amount, 2)', new_text: 'return round(max(0.0, amount - discount_amount), 2)'}]},
        {kind: 'edit', path: 'tests/test_checkout.py', edits: [{kind: 'insert_before', anchor_text: '    def test_zero_discount(self):', new_text: '    def test_fixed_discount_larger_than_subtotal_clamps_at_zero(self):\n        cart = Cart([LineItem("Adapter", 25.0)])\n        summary = CheckoutService().calculate(cart, discount_amount=40.0, tax_rate=0.1)\n\n        self.assertEqual(summary.subtotal, 25.0)\n        self.assertEqual(summary.discounted_subtotal, 0.0)\n        self.assertEqual(summary.tax, 0.0)\n        self.assertEqual(summary.total, 0.0)\n\n'}]},
    ];
    if (provider === 'webcodex') {
        changes.forEach(change => {
            change.expected_sha256 = read.items.find(item => item.path === change.path).output.sha256;
            if (!change.expected_sha256) throw Error('missing guarded read digest');
        });
    }
    await call('apply_text_edits', {changes});
    const test = await call('run_process', {executable:'python3', args:['-B','-m','unittest','discover','-s','tests','-v'], timeout_secs:30, sync_wait_secs:30, purpose:'validation'});
    const review = await call('show_changes', {include_diff:false, session_event_limit:0});
    return {provider, samples, total_ms:samples.reduce((sum, row) => sum + row.ms, 0), test, review};
}
