const CONFIG = {
warmupRuns:  3,
measureRuns: 5,
}

// Benchmark payloads are preallocated to avoid allocations during measurement
const PAYLOADS = Object.freeze({
burst:   new Uint8Array(1_024),
medium:  new Uint8Array(1_000_000),
bulk:    new Uint8Array(5_000_000),
heavy:   new Uint8Array(10_000_000),
extreme: new Uint8Array(20_000_000),
})

// Benchmark workloads
//
// Each test opens a single stream, fires all writes without awaiting,
// then signals end. Timing runs from first write until all echoed
// data is received via the onData callback — matching the proven
// pattern in tests/stream/app.js.
//
// Responses are intentionally not retained: no forced GC, so V8 collects
// on its own schedule. This keeps conditions realistic for comparison
// against other runtimes.
async function streamBurst() {
const payload = PAYLOADS.burst
const count   = 1000
const expectedBytes = payload.byteLength * count
const stream  = await kurogane.openStream('echo', '')
let receivedBytes = 0
const allEchoed = new Promise(resolve => {
    stream.onData(data => {
    receivedBytes += data.byteLength
    if (receivedBytes >= expectedBytes) resolve()
    })
    stream.onError(err => { throw new Error(err) })
})
const t0 = performance.now()
for (let i = 0; i < count; i++) stream.write(payload)
stream.end('')
await allEchoed
return performance.now() - t0
}

async function streamMedium() {
const payload = PAYLOADS.medium
const count   = 100
const expectedBytes = payload.byteLength * count
const stream  = await kurogane.openStream('echo', '')
let receivedBytes = 0
const allEchoed = new Promise(resolve => {
    stream.onData(data => {
    receivedBytes += data.byteLength
    if (receivedBytes >= expectedBytes) resolve()
    })
    stream.onError(err => { throw new Error(err) })
})
const t0 = performance.now()
for (let i = 0; i < count; i++) stream.write(payload)
stream.end('')
await allEchoed
return performance.now() - t0
}

async function streamBulk() {
const payload = PAYLOADS.bulk
const count   = 100
const expectedBytes = payload.byteLength * count
const stream  = await kurogane.openStream('echo', '')
let receivedBytes = 0
const allEchoed = new Promise(resolve => {
    stream.onData(data => {
    receivedBytes += data.byteLength
    if (receivedBytes >= expectedBytes) resolve()
    })
    stream.onError(err => { throw new Error(err) })
})
const t0 = performance.now()
for (let i = 0; i < count; i++) stream.write(payload)
stream.end('')
await allEchoed
return performance.now() - t0
}

async function streamHeavy() {
const payload = PAYLOADS.heavy
const count   = 50
const expectedBytes = payload.byteLength * count
const stream  = await kurogane.openStream('echo', '')
let receivedBytes = 0
const allEchoed = new Promise(resolve => {
    stream.onData(data => {
    receivedBytes += data.byteLength
    if (receivedBytes >= expectedBytes) resolve()
    })
    stream.onError(err => { throw new Error(err) })
})
const t0 = performance.now()
for (let i = 0; i < count; i++) stream.write(payload)
stream.end('')
await allEchoed
return performance.now() - t0
}

async function streamExtreme() {
const payload = PAYLOADS.extreme
const count   = 100
const expectedBytes = payload.byteLength * count
const stream  = await kurogane.openStream('echo', '')
let receivedBytes = 0
const allEchoed = new Promise(resolve => {
    stream.onData(data => {
    receivedBytes += data.byteLength
    if (receivedBytes >= expectedBytes) resolve()
    })
    stream.onError(err => { throw new Error(err) })
})
const t0 = performance.now()
for (let i = 0; i < count; i++) stream.write(payload)
stream.end('')
await allEchoed
return performance.now() - t0
}

// Deterministic suite ordering for consistent comparisons
// rtBytes = aggregate round-trip transfer volume (2x because echo)
const SUITE = [
{ id: 'stream_burst',  name: 'Burst transfer',     description: '1 KB × 1,000 via stream pipeline', fn: streamBurst,  rtBytes: 2 * 1_024 * 1_000 },
{ id: 'stream_medium', name: 'Stream payload',      description: '1 MB × 100 via stream pipeline',   fn: streamMedium, rtBytes: 2 * 1_000_000 * 100 },
{ id: 'stream_bulk',   name: 'Bulk transport',      description: '5 MB × 100 via stream pipeline',   fn: streamBulk,   rtBytes: 2 * 5_000_000 * 100 },
{ id: 'stream_heavy',  name: 'High-pressure transport', description: '10 MB × 50 via stream pipeline',  fn: streamHeavy,  rtBytes: 2 * 10_000_000 * 50 },
{ id: 'stream_extreme', name: 'Sustained saturation', description: '20 MB × 100 via stream pipeline', fn: streamExtreme, rtBytes: 2 * 20_000_000 * 100 },
]

// Benchmark utilities
const sleep = ms => new Promise(r => setTimeout(r, ms))

/** @param {number[]} values @returns {number} */
function median(values) {
const s = [...values].sort((a, b) => a - b)
const m = Math.floor(s.length / 2)
return s.length % 2 === 0 ? (s[m - 1] + s[m]) / 2 : s[m]
}

/** @param {number[]} values @param {number} p @returns {number} */
function percentile(values, p) {
const s   = [...values].sort((a, b) => a - b)
const idx = Math.max(0, Math.ceil((p / 100) * s.length) - 1)
return s[idx]
}

/** @param {number} rtBytes @param {number} elapsedMs @returns {number} MB/s */
function throughputMBs(rtBytes, elapsedMs) {
return (rtBytes / 1e6) / (elapsedMs / 1000)
}

/** @param {string} tag @param {string} className @param {string} [text] */
function el(tag, className, text) {
const e = document.createElement(tag)
e.className = className
if (text !== undefined) e.textContent = text
return e
}

/** @returns {{ row: HTMLElement, value: HTMLElement }} */
function makeResultRow(label) {
const row   = el('div', 'result-row')
const value = el('span', 'result-value')
row.append(el('span', 'result-label', label), value)
return { row, value }
}


// UI controller
//
// DOM references are cached during initialization. Benchmark cards expose
// lightweight update hooks for phase transitions and completion state.
class BenchmarkUI {
constructor() {
    this._logEl        = document.getElementById('console-log')
    this._overallPanel = document.getElementById('overall-panel')
    this._overallBar   = document.getElementById('overall-bar')
    this._overallPct   = document.getElementById('overall-pct')
    this._summaryPanel = document.getElementById('summary-panel')
    this._summaryStats = document.getElementById('summary-stats')
    this._summaryTable = document.getElementById('summary-table')
    this._cardGrid     = document.getElementById('card-grid')
    this._copyBtn      = document.getElementById('copy-btn')
    this._copyOk       = document.getElementById('copy-ok')
    this._exportJson   = ''

    document.getElementById('si-engine').textContent =
    navigator.userAgent.match(/Chrome\/[\d.]+/)?.[0] ?? navigator.userAgent.slice(0, 48)
    document.getElementById('si-cores').textContent =
    String(navigator.hardwareConcurrency)

    // Console toggle
    const toggle  = document.getElementById('console-toggle')
    const wrap    = document.getElementById('console-wrap')
    const chevron = document.getElementById('console-chevron')
    toggle.addEventListener('click', () => {
    chevron.classList.toggle('open', wrap.classList.toggle('visible'))
    })

    // Copy button
    this._copyBtn.addEventListener('click', async () => {
    await navigator.clipboard.writeText(this._exportJson)
    this._copyOk.classList.add('visible')
    setTimeout(() => this._copyOk.classList.remove('visible'), 2000)
    })
}

log(line) {
    console.log(line)
    this._logEl.appendChild(document.createTextNode(line + '\n'))
    this._logEl.parentElement.scrollTop = this._logEl.parentElement.scrollHeight
}

setOverall(done, total) {
    this._overallPanel.classList.add('visible')
    this._overallBar.style.width = `${Math.round((done / total) * 100)}%`
    this._overallPct.textContent = `${done} / ${total}`
    if (done === total) this._overallBar.classList.add('complete')
}

/**
 * Build a card for one test. Appends it to the grid and returns
 * { onPhase, onDone } callbacks for the runner to drive.
 */
makeCard(test) {
    const card = el('div', 'test-card')

    // header
    const header = el('div', 'card-header')
    const meta   = el('div', '')
    const nameEl = el('div', 'card-name', test.name)
    const descEl = el('div', 'card-desc', test.description)
    meta.append(nameEl, descEl)
    const badge = el('span', 'badge pending', 'pending')
    header.append(meta, badge)

    // progress
    const progress  = el('div', 'card-progress')
    const phHeader  = el('div', 'card-progress-header')
    const stageEl   = el('span', 'card-stage')
    const pctEl     = el('span', 'card-pct')
    phHeader.append(stageEl, pctEl)
    const track = el('div', 'bar-track')
    const fill  = el('div', 'bar-fill')
    track.appendChild(fill)
    progress.append(phHeader, track)

    // results
    const results  = el('div', 'card-results')
    const medRow   = makeResultRow('Median')
    const p95Row   = makeResultRow('P95')
    const tpRow    = test.rtBytes != null ? makeResultRow('Throughput') : null
    const divider  = el('hr', 'result-divider')
    const details  = document.createElement('details')
    const summary  = el('summary', '', 'View runs')
    const runsList = el('div', 'runs-list')
    details.append(summary, runsList)
    results.append(medRow.row, p95Row.row)
    if (tpRow) results.append(tpRow.row)
    results.append(divider, details)

    card.append(header, progress, results)
    this._cardGrid.appendChild(card)

    // callbacks
    function onPhase(stage, i, total) {
    const warming = stage === 'warmup'
    const pct     = warming ? ((i + 1) / total) * 50 : 50 + ((i + 1) / total) * 50

    card.classList.add('state-active')
    badge.className   = `badge ${warming ? 'warming' : 'measuring'}`
    badge.textContent = warming ? 'warming' : 'measuring'
    progress.classList.add('visible')
    stageEl.textContent = warming
        ? `Warming up... (${i + 1}/${total})`
        : `Measuring... (${i + 1}/${total})`
    pctEl.textContent   = `${Math.round(pct)}%`
    fill.style.width    = `${pct}%`
    }

    function onDone(data) {
    card.classList.remove('state-active')
    card.classList.add('state-done')
    badge.className   = 'badge done'
    badge.textContent = 'done'
    progress.classList.remove('visible')
    results.classList.add('visible')

    medRow.value.textContent = `${data.median.toFixed(2)} ms`
    p95Row.value.textContent = `${data.p95.toFixed(2)} ms`

    if (tpRow && data.tp != null) {
        tpRow.value.textContent = `${data.tp.toFixed(2)} MB/s`
        tpRow.value.classList.add('accent')
    }

    data.runs.forEach((t, i) => {
        const row   = el('div', 'run-row')
        const label = el('span', 'run-label', `Run ${i + 1}`)
        const val   = el('span', '', `${t.toFixed(2)} ms`)
        row.append(label, val)
        runsList.appendChild(row)
    })
    }

    return { onPhase, onDone }
}

showSummary(allResults, exportJson) {
    this._exportJson = exportJson

    const entries = SUITE.map(t => ({ test: t, r: allResults[t.id] }))
    const avg     = entries.reduce((s, { r }) => s + r.median, 0) / entries.length
    const sorted  = [...entries].sort((a, b) => a.r.median - b.r.median)
    const fastest = sorted[0]
    const slowest = sorted[sorted.length - 1]

    // stat boxes
    this._summaryStats.innerHTML = ''

    const makeStatBox = (label, primary, colorClass, sub) => {
    const box = el('div', 'stat-box')
    box.append(
        el('div', 'stat-label', label),
        el('div', `stat-primary${colorClass ? ' ' + colorClass : ''}`, primary),
        ...(sub ? [el('div', 'stat-sub', sub)] : [])
    )
    return box
    }

    this._summaryStats.append(
    makeStatBox('avg median',  `${avg.toFixed(2)} ms`,           null,    null),
    makeStatBox('fastest',      fastest.test.name,                'green', `${fastest.r.median.toFixed(2)} ms`),
    makeStatBox('slowest',      slowest.test.name,                'amber', `${slowest.r.median.toFixed(2)} ms`)
    )

    // sorted table
    this._summaryTable.innerHTML = ''
    for (const { test, r } of sorted) {
    const row  = el('div', 'summary-row')
    const tp   = r.tp != null ? `${r.tp.toFixed(1)} MB/s` : '—'
    const range = `min ${r.min.toFixed(1)}  max ${r.max.toFixed(1)}`
    row.append(
        el('div', 'sc sc-name',  test.name),
        el('div', 'sc sc-med',   `${r.median.toFixed(2)} ms`),
        el('div', 'sc sc-tp',    tp),
        el('div', 'sc sc-range', range)
    )
    this._summaryTable.appendChild(row)
    }

    this._summaryPanel.classList.add('visible')
    this._summaryPanel.scrollIntoView({ behavior: 'smooth', block: 'start' })
}
}

// Runner
async function runTest(test, card, ui) {
ui.log(`\n── ${test.name}`)

for (let i = 0; i < CONFIG.warmupRuns; i++) {
    card.onPhase('warmup', i, CONFIG.warmupRuns)
    ui.log(`  warmup ${i + 1}/${CONFIG.warmupRuns}`)
    await test.fn()
    await sleep(250)
}

await sleep(500)

const times = []

for (let i = 0; i < CONFIG.measureRuns; i++) {
    card.onPhase('measure', i, CONFIG.measureRuns)

    // yield to flush any pending DOM layout/paint before the clock starts
    await sleep(0)

    const t0 = performance.now()
    await test.fn()
    const dt = performance.now() - t0

    times.push(dt)
    ui.log(`  run ${i + 1}: ${dt.toFixed(2)} ms`)

    await sleep(500)
}

const med = median(times)
const p95 = percentile(times, 95)
const tp  = test.rtBytes != null ? throughputMBs(test.rtBytes, med) : null
const min = Math.min(...times)
const max = Math.max(...times)

ui.log(`  median ${med.toFixed(2)} ms  p95 ${p95.toFixed(2)} ms`)
if (tp != null) ui.log(`  throughput ${tp.toFixed(2)} MB/s`)

const result = { runs: times, median: med, p95, tp, min, max }
card.onDone(result)
return result
}

async function main() {
const ui      = new BenchmarkUI()
const cards   = SUITE.map(test => ({ test, card: ui.makeCard(test) }))
const results = {}

ui.log(`ua:    ${navigator.userAgent}`)
ui.log(`cores: ${navigator.hardwareConcurrency}`)

// Untimed warmup pass.
//
// Stabilizes JIT compilation, IPC buffering and scheduler behavior before
// benchmark measurements begin.
ui.log('\nGlobal warmup pass...')
for (const { test } of cards) await test.fn()
await sleep(1000)

for (let i = 0; i < cards.length; i++) {
    const { test, card } = cards[i]
    ui.setOverall(i, cards.length)
    results[test.id] = await runTest(test, card, ui)
}

ui.setOverall(cards.length, cards.length)

const exportJson = JSON.stringify(
    Object.fromEntries(
    SUITE.map(({ id }) => {
        const r = results[id]
        return [id, {
        runs:          r.runs,
        median:        r.median,
        p95:           r.p95,
        min:           r.min,
        max:           r.max,
        throughput:    r.tp,
        }]
    })
    ),
    null, 2
)

ui.showSummary(results, exportJson)
}

main()
