const canvas = document.getElementById("gl");
const hud = document.getElementById("hud");

const gl = canvas.getContext("webgl2", {
  antialias: false,
  depth: false,
  stencil: false,
  powerPreference: "high-performance",
});

if (!gl) {
  hud.textContent = "WebGL2 not supported";
  throw new Error("WebGL2 not supported");
}

function resize() {
  const dpr = window.devicePixelRatio || 1;
  canvas.width = innerWidth * dpr;
  canvas.height = innerHeight * dpr;
  canvas.style.width = innerWidth + "px";
  canvas.style.height = innerHeight + "px";
  gl.viewport(0, 0, canvas.width, canvas.height);
}
addEventListener("resize", resize);
resize();

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

let COUNT = 100_000;

const inst = new Float32Array(COUNT * 3);
for (let i = 0; i < COUNT; i++) {
  inst[i*3+0] = Math.random() * canvas.width;
  inst[i*3+1] = Math.random() * canvas.height;
  inst[i*3+2] = Math.random() * 10.0;
}

const instBuf = gl.createBuffer();
gl.bindBuffer(gl.ARRAY_BUFFER, instBuf);
gl.bufferData(gl.ARRAY_BUFFER, inst, gl.STATIC_DRAW);

gl.enableVertexAttribArray(1);
gl.vertexAttribPointer(1, 2, gl.FLOAT, false, 12, 0);
gl.vertexAttribDivisor(1, 1);

gl.enableVertexAttribArray(2);
gl.vertexAttribPointer(2, 1, gl.FLOAT, false, 12, 8);
gl.vertexAttribDivisor(2, 1);

const uTime = gl.getUniformLocation(prog, "uTime");
const uRes = gl.getUniformLocation(prog, "uRes");

let last = performance.now();
let frames = 0;
let avgDt = 0;

function frame(time) {
  frames++;

  const dt = time - last;
  last = time;

  // exponential moving average for stable numbers
  avgDt = avgDt * 0.9 + dt * 0.1;

  if (frames % 30 === 0) {
    hud.textContent =
      `Instances: ${COUNT.toLocaleString()} | ` +
      `FPS: ${(1000 / avgDt).toFixed(1)} | ` +
      `Δ ${avgDt.toFixed(2)}ms`;
  }

  gl.clear(gl.COLOR_BUFFER_BIT);

  gl.uniform1f(uTime, time * 0.001);
  gl.uniform2f(uRes, canvas.width, canvas.height);

  gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, 4, COUNT);

  requestAnimationFrame(frame);
}

requestAnimationFrame(frame);
