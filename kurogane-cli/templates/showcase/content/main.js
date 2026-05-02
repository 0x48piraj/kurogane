const tabBar = document.getElementById("tab-bar");
const hud = document.getElementById("hud");
const badge = document.getElementById("mode-badge");
const c2d = document.getElementById("canvas-2d");
const cgl = document.getElementById("canvas-gl");

const state = {
  activeTab: "canvas",
  canvasPaused: false,
  glPaused: true,
  glReady: false,
};

let frameCanvas = null;
let frameGL = null;

function switchTab(tab) {
  if (tab === state.activeTab) return;
  state.activeTab = tab;

  tabBar.dataset.active = tab;
  badge.textContent = tab === "canvas" ? "Canvas 2D" : "WebGL 2";

  if (tab === "canvas") {
    cgl.classList.remove("visible");
    c2d.classList.add("visible");

    state.glPaused = true;

    if (state.canvasPaused && frameCanvas) {
      state.canvasPaused = false;
      requestAnimationFrame(frameCanvas);
    }
  } else {
    if (!state.glReady) return;

    c2d.classList.remove("visible");
    cgl.classList.add("visible");

    state.canvasPaused = true;

    if (state.glPaused && frameGL) {
      state.glPaused = false;
      requestAnimationFrame(frameGL);
    }
  }
}

document.querySelectorAll(".tab-btn").forEach(btn => {
  btn.addEventListener("click", () => switchTab(btn.dataset.tab));
});

/* CANVAS 2D RENDERER */
{
  const ctx = c2d.getContext("2d", { alpha: false });
  const fpsEl = hud;

  function resizeCanvas() {
    const dpr = devicePixelRatio || 1;
    const W = innerWidth;
    const H = innerHeight - 52;

    c2d.width = W * dpr;
    c2d.height = H * dpr;
    c2d.style.width = W + "px";
    c2d.style.height = H + "px";
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  }

  addEventListener("resize", resizeCanvas);
  resizeCanvas();

  let COUNT = 5000; // more boxes, more fun
  const SIZE = 6;

  let x;
  let y;
  let vx;
  let vy;
  let hue;

  // cache-friendly structure-of-arrays
  function buildCanvas() {
    const dpr = devicePixelRatio || 1;
    const W = c2d.width / dpr;
    const H = c2d.height / dpr;

    x = new Float32Array(COUNT);
    y = new Float32Array(COUNT);
    vx = new Float32Array(COUNT);
    vy = new Float32Array(COUNT);
    hue = new Float32Array(COUNT);

    for (let i = 0; i < COUNT; i++) {
      x[i] = Math.random() * W;
      y[i] = Math.random() * H;
      vx[i] = Math.random() * 2 - 1; // speed [-1, +1)
      vy[i] = Math.random() * 2 - 1;
      hue[i] = Math.random() * 360;
    }
  }

  buildCanvas();

  let last = performance.now();
  let frames = 0;
  let avgDt = 0;

  frameCanvas = function frameCanvas(time) {
    if (state.canvasPaused) return;

    frames++;

    const dt = time - last;
    last = time;
    avgDt = avgDt * 0.9 + dt * 0.1;

    if (frames % 30 === 0 && state.activeTab === "canvas") {
      fpsEl.textContent =
        `Instances: ${COUNT.toLocaleString()} | ` +
        `FPS: ${(1000 / avgDt).toFixed(1)} | ` +
        `Δ ${avgDt.toFixed(2)}ms`;
    }

    const dpr = devicePixelRatio || 1;
    const W = c2d.width / dpr;
    const H = c2d.height / dpr;

    // trail fade instead of full clear (cheaper)
    ctx.fillStyle = "rgba(5, 8, 20, 0.25)";
    ctx.fillRect(0, 0, W, H);

    // batch by hue bands (reduces fillStyle changes)
    for (let band = 0; band < 6; band++) {
      ctx.fillStyle = `hsl(${band * 60}, 80%, 60%)`;

      for (let i = band; i < COUNT; i += 6) {
        let nx = x[i] + vx[i];
        let ny = y[i] + vy[i];

        // bounce off edges
        if (nx < 0 || nx > W) vx[i] *= -1;
        if (ny < 0 || ny > H) vy[i] *= -1;

        x[i] += vx[i];
        y[i] += vy[i];

        ctx.fillRect(x[i], y[i], SIZE, SIZE);
      }
    }

    requestAnimationFrame(frameCanvas);
  };

  state.canvasPaused = false;
  requestAnimationFrame(frameCanvas);

  // controls
  addEventListener("keydown", e => {
    if (e.code === "Equal" || e.code === "NumpadAdd") {
      COUNT = Math.min(COUNT * 2, 100_000);
      buildCanvas();
    }

    if (
      (e.code === "Minus" || e.code === "NumpadSubtract") &&
      COUNT > 1000
    ) {
      COUNT >>= 1;
      buildCanvas();
    }
  });

  // expose for tab switching / resizing
  window._buildCanvas = buildCanvas;
  window._canvasCOUNT = () => COUNT;
  window._setCanvasCOUNT = v => {
    COUNT = v;
    buildCanvas();
  };
}

/* WEBGL2 RENDERER */
{
  const gl = cgl.getContext("webgl2", {
    antialias: false,
    depth: false,
    stencil: false,
    powerPreference: "high-performance",
  });

  if (!gl) {
    document.querySelector('.tab-btn[data-tab="webgl2"]').style.opacity = "0.35";
    hud.textContent = "WebGL2 not supported";
  } else {
    state.glReady = true;

    function resizeGL() {
      const dpr = devicePixelRatio || 1;
      const W = innerWidth;
      const H = innerHeight - 52;

      cgl.width = W * dpr;
      cgl.height = H * dpr;
      cgl.style.width = W + "px";
      cgl.style.height = H + "px";
      gl.viewport(0, 0, cgl.width, cgl.height);
    }

    addEventListener("resize", resizeGL);
    resizeGL();

    // state
    gl.disable(gl.DEPTH_TEST);
    gl.disable(gl.BLEND);
    gl.clearColor(0.02, 0.02, 0.05, 1.0);

    // shaders
    const vs = `#version 300 es
precision highp float;

layout(location=0) in vec2 quad;
layout(location=1) in vec2 pos;
layout(location=2) in float phase;

uniform float uTime;
uniform vec2 uRes;

out float vPhase;

void main() {
  float t = uTime + phase;

  vec2 p = pos + vec2(
    sin(t * 1.3),
    cos(t * 0.9)
  ) * 30.0;

  vPhase = phase;

  vec2 clip = (p + quad) / uRes * 2.0 - 1.0;
  clip.y = -clip.y;

  gl_Position = vec4(clip, 0.0, 1.0);
}
`;

    const fs = `#version 300 es
precision highp float;

in float vPhase;
out vec4 outColor;

vec3 hsv(float h, float s, float v) {
  vec3 rgb = clamp(
    abs(mod(h * 6.0 + vec3(0,4,2), 6.0) - 3.0) - 1.0,
    0.0,
    1.0
  );
  return v * mix(vec3(1.0), rgb, s);
}

void main() {
  float h = fract(vPhase * 0.1);
  vec3 c = hsv(h, 0.9, 0.95);
  outColor = vec4(c, 1.0);
}
`;

    function compile(type, src) {
      const s = gl.createShader(type);
      gl.shaderSource(s, src);
      gl.compileShader(s);
      if (!gl.getShaderParameter(s, gl.COMPILE_STATUS))
        throw gl.getShaderInfoLog(s);
      return s;
    }

    const prog = gl.createProgram();
    gl.attachShader(prog, compile(gl.VERTEX_SHADER, vs));
    gl.attachShader(prog, compile(gl.FRAGMENT_SHADER, fs));
    gl.linkProgram(prog);
    if (!gl.getProgramParameter(prog, gl.LINK_STATUS))
      throw gl.getProgramInfoLog(prog);

    gl.useProgram(prog);

    const quad = new Float32Array([
      -6, -6,
       6, -6,
      -6,  6,
       6,  6,
    ]);

    const quadBuf = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, quadBuf);
    gl.bufferData(gl.ARRAY_BUFFER, quad, gl.STATIC_DRAW);
    gl.enableVertexAttribArray(0);
    gl.vertexAttribPointer(0, 2, gl.FLOAT, false, 0, 0);

    let GL_COUNT = 100_000;
    let instBuf;

    function buildGL() {
      const inst = new Float32Array(GL_COUNT * 3);
      for (let i = 0; i < GL_COUNT; i++) {
        inst[i * 3 + 0] = Math.random() * cgl.width;
        inst[i * 3 + 1] = Math.random() * cgl.height;
        inst[i * 3 + 2] = Math.random() * 10.0;
      }

      if (!instBuf) instBuf = gl.createBuffer();
      gl.bindBuffer(gl.ARRAY_BUFFER, instBuf);
      gl.bufferData(gl.ARRAY_BUFFER, inst, gl.STATIC_DRAW);

      gl.enableVertexAttribArray(1);
      gl.vertexAttribPointer(1, 2, gl.FLOAT, false, 12, 0);
      gl.vertexAttribDivisor(1, 1);

      gl.enableVertexAttribArray(2);
      gl.vertexAttribPointer(2, 1, gl.FLOAT, false, 12, 8);
      gl.vertexAttribDivisor(2, 1);
    }

    buildGL();

    const uTime = gl.getUniformLocation(prog, "uTime");
    const uRes = gl.getUniformLocation(prog, "uRes");

    let lastGL = performance.now();
    let framesGL = 0;
    let avgDtGL = 0;

    frameGL = function frameGL(time) {
      if (state.glPaused) return;

      framesGL++;

      const dt = time - lastGL;
      lastGL = time;

      // exponential moving average for stable numbers
      avgDtGL = avgDtGL * 0.9 + dt * 0.1;

      if (framesGL % 30 === 0 && state.activeTab === "webgl2") {
        hud.textContent =
          `Instances: ${GL_COUNT.toLocaleString()} | ` +
          `FPS: ${(1000 / avgDtGL).toFixed(1)} | ` +
          `Δ ${avgDtGL.toFixed(2)}ms`;
      }

      gl.clear(gl.COLOR_BUFFER_BIT);

      gl.uniform1f(uTime, time * 0.001);
      gl.uniform2f(uRes, cgl.width, cgl.height);

      gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, 4, GL_COUNT);

      requestAnimationFrame(frameGL);
    };

    window._glCOUNT = () => GL_COUNT;
    window._setGLCOUNT = v => {
      GL_COUNT = v;
      buildGL();
    };

    // Start paused, only animate when tab is visible
    state.glPaused = true;
  }
}

/* KEYBOARD CONTROLS  (+/-) */
addEventListener("keydown", e => {
  const plus = e.code === "Equal" || e.code === "NumpadAdd";
  const minus = e.code === "Minus" || e.code === "NumpadSubtract";
  if (!plus && !minus) return;

  if (state.activeTab === "canvas") {
    let c = window._canvasCOUNT();
    c = plus ? Math.min(c * 2, 200_000) : Math.max(c >> 1, 500);
    window._setCanvasCOUNT(c);
  } else {
    let c = window._glCOUNT?.() ?? 100000;
    c = plus ? Math.min(c * 2, 2_000_000) : Math.max(c >> 1, 1000);
    window._setGLCOUNT?.(c);
  }
});