import { createEffect, createSignal, onCleanup, onMount, For, Show } from 'solid-js';
import uPlot from 'uplot';
import 'uplot/dist/uPlot.min.css';

export default function Chart(props) {
  let host;
  const [ready, setReady] = createSignal(false);
  let plot;
  let observer;

  const draw = (u) => {
    const { ctx, bbox } = u;
    ctx.save();
    for (const overlay of props.overlays ?? []) {
      const left = u.valToPos(overlay.from, 'x', true);
      const right = u.valToPos(overlay.to, 'x', true);
      ctx.fillStyle = 'rgba(94, 234, 212, 0.09)';
      ctx.fillRect(left, bbox.top, Math.max(2, right - left), bbox.height);
    }
    if (props.limit !== null && props.limit !== undefined) {
      const y = u.valToPos(props.limit, 'y', true);
      ctx.strokeStyle = '#fb7185';
      ctx.setLineDash([6, 4]);
      ctx.lineWidth = 1.5;
      ctx.beginPath();
      ctx.moveTo(bbox.left, y);
      ctx.lineTo(bbox.left + bbox.width, y);
      ctx.stroke();
    }
    ctx.restore();
  };

  onMount(() => {
    const options = {
      title: props.title,
      width: Math.max(320, host.clientWidth || 640),
      height: props.height ?? 220,
      padding: [14, 12, 0, 0],
      scales: { x: { time: false } },
      axes: [
        { stroke: '#8b98a9', grid: { stroke: 'rgba(148, 163, 184, 0.12)' }, values: (_, ticks) => ticks.map((tick) => new Date(tick).toISOString().slice(11, 19)) },
        { stroke: '#8b98a9', grid: { stroke: 'rgba(148, 163, 184, 0.12)' } },
      ],
      series: [
        { label: 'time' },
        ...(props.series ?? []).map((entry) => ({ label: entry.label, stroke: entry.color, width: 2, points: { show: false } })),
      ],
      legend: { show: true },
      hooks: { draw: [draw] },
    };
    plot = new uPlot(options, props.data ?? [], host);
    setReady(true);
    observer = new ResizeObserver(() => {
      if (host.clientWidth > 0) plot.setSize({ width: host.clientWidth, height: props.height ?? 220 });
    });
    observer.observe(host);
  });

  createEffect(() => {
    const data = props.data;
    const limit = props.limit;
    const overlays = props.overlays;
    if (!ready()) return;
    plot.setData(data ?? []);
    plot.redraw();
    void limit;
    void overlays;
  });

  onCleanup(() => {
    observer?.disconnect();
    plot?.destroy();
  });

  return (
    <section class="panel chart-panel">
      <div class="panel-head">
        <h3>{props.title}</h3>
        <For each={props.series ?? []}>{(entry) => <span class="legend" style={{ color: entry.color }}>{entry.label}</span>}</For>
      </div>
      <div class="chart-host" ref={host}></div>
      <Show when={props.limit !== null && props.limit !== undefined}>
        <p class="chart-note">dashed red line = configured limit</p>
      </Show>
    </section>
  );
}
