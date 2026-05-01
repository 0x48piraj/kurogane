const canvas = document.getElementById("canvas");
const ctx = canvas.getContext("2d");
const fpsEl = document.getElementById("fps");

function resize() {
  canvas.width = window.innerWidth;
  canvas.height = window.innerHeight;
}
window.addEventListener("resize", resize);
resize();

const BOX_COUNT = 1000; // more boxes, more fun
const SIZE = 16;

const boxes = Array.from({ length: BOX_COUNT }, () => ({
  x: Math.random() * canvas.width,
  y: Math.random() * canvas.height,
  vx: Math.random() * 4 - 2, // speed [-2, +2)
  vy: Math.random() * 4 - 2,
  color: `hsl(${Math.random() * 360}, 70%, 60%)`
}));

let lastTime = performance.now();
let frames = 0;

function frame(time) {
  frames++;

  if (time - lastTime >= 1000) {
    fpsEl.textContent = `FPS: ${frames}`;
    frames = 0;
    lastTime = time;
  }

  ctx.clearRect(0, 0, canvas.width, canvas.height);

  for (const b of boxes) {
    b.x += b.vx;
    b.y += b.vy;

    // bounce off edges
    if (b.x < 0 || b.x > canvas.width) b.vx *= -1;
    if (b.y < 0 || b.y > canvas.height) b.vy *= -1;

    ctx.fillStyle = b.color;
    ctx.fillRect(b.x, b.y, SIZE, SIZE);
  }

  requestAnimationFrame(frame);
}

requestAnimationFrame(frame);
