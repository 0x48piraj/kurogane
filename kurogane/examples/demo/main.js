const result = await kurogane.invoke("add", {a:1,b:2});
console.log(result);

const canvas = document.getElementById("canvas");
const ctx = canvas.getContext("2d", { alpha: false });
const fpsEl = document.getElementById("fps");

function resize() {
  const dpr = window.devicePixelRatio || 1;
  canvas.width = innerWidth * dpr;
  canvas.height = innerHeight * dpr;
  canvas.style.width = innerWidth + "px";
  canvas.style.height = innerHeight + "px";
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
}
addEventListener("resize", resize);
resize();

let COUNT = 5000; // more boxes, more fun
const SIZE = 6;

// cache-friendly structure-of-arrays
const x  = new Float32Array(COUNT);
const y  = new Float32Array(COUNT);
const vx = new Float32Array(COUNT);
const vy = new Float32Array(COUNT);
const hue = new Float32Array(COUNT);

for (let i = 0; i < COUNT; i++) {
  x[i] = Math.random() * canvas.width;
  y[i] = Math.random() * canvas.height;
  vx[i] = Math.random() * 2 - 1; // speed [-1, +1)
  vy[i] = Math.random() * 2 - 1;
  hue[i] = Math.random() * 360;
}

let last = performance.now();
let frames = 0;
let avgDt = 0;

function frame(time) {
  frames++;

  const dt = time - last;
  last = time;
  avgDt = avgDt * 0.9 + dt * 0.1;

  if (frames % 30 === 0) {
    fpsEl.textContent =
      `Instances: ${COUNT.toLocaleString()} | ` +
      `FPS: ${(1000 / avgDt).toFixed(1)} | ` +
      `Δ ${avgDt.toFixed(2)}ms`;
  }

  // trail fade instead of full clear (cheaper)
  ctx.fillStyle = "rgba(5, 8, 20, 0.25)";
  ctx.fillRect(0, 0, canvas.width, canvas.height);

  // batch by hue bands (reduces fillStyle changes)
  for (let band = 0; band < 6; band++) {
    ctx.fillStyle = `hsl(${band * 60}, 80%, 60%)`;

    for (let i = band; i < COUNT; i += 6) {
      let nx = x[i] + vx[i];
      let ny = y[i] + vy[i];

      // bounce off edges
      if (nx < 0 || nx > canvas.width)  vx[i] *= -1;
      if (ny < 0 || ny > canvas.height) vy[i] *= -1;

      x[i] += vx[i];
      y[i] += vy[i];

      ctx.fillRect(x[i], y[i], SIZE, SIZE);
    }
  }

  requestAnimationFrame(frame);
}

requestAnimationFrame(frame);

// controls
addEventListener("keydown", e => {
  if (e.code === "Equal" || e.code === "NumpadAdd") {
    COUNT = Math.min(COUNT * 2, 100_000);
    buildInstances?.();
  }

  if (
    (e.code === "Minus" || e.code === "NumpadSubtract") &&
    COUNT > 1000
  ) {
    COUNT >>= 1;
    buildInstances?.();
  }
});
