const canvas = document.getElementById("canvas");
const ctx = canvas.getContext("2d");
const fpsEl = document.getElementById("fps");

function resize() {
  canvas.width = window.innerWidth;
  canvas.height = window.innerHeight;
}
window.addEventListener("resize", resize);
resize();

const imports = {
  env: {
    clear: () => {
      ctx.clearRect(0, 0, canvas.width, canvas.height);
    },
    draw_rect: (x, y, size, r, g, b) => {
      ctx.fillStyle = `rgb(${r}, ${g}, ${b})`;
      ctx.fillRect(x, y, size, size);
    },
    width: () => canvas.width,
    height: () => canvas.height,
  }
};

const wasm = await WebAssembly.instantiateStreaming(
  fetch("app://app/demo.wasm"),
  imports
);

const { init, tick } = wasm.instance.exports;

init(); // initialize boxes inside WASM

let lastTime = performance.now();
let frames = 0;

function frame(time) {
  frames++;

  if (time - lastTime >= 1000) {
    fpsEl.textContent = `FPS: ${frames}`;
    frames = 0;
    lastTime = time;
  }

  tick();
  requestAnimationFrame(frame);
}

requestAnimationFrame(frame);
