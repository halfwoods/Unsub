import { COMMON_GLSL, FULLSCREEN_VS, Program, VERSION, bindTexture, type StyleHost } from "../gl";
import type { RenderInfo, StyleRenderer } from "../styles";

const FS = `${VERSION}
${COMMON_GLSL}
uniform sampler2D uArt;
uniform vec2 uRes;
uniform vec3 uTint;
uniform float uTime, uRms, uBeat, uKick, uLow, uAppear;
in vec2 vUv;
out vec4 o;

float roundedBox(vec2 p, vec2 b, float r) {
  vec2 d = abs(p) - b + r;
  return length(max(d, 0.0)) + min(max(d.x, d.y), 0.0) - r;
}

void main() {
  float aspect = uRes.x / uRes.y;
  vec2 p = (vUv - 0.5) * vec2(aspect, 1.0);

  // The backdrop is the cover itself, blown up, blurred and darkened, with the tint pushed in.
  vec2 drift = vec2(sin(uTime * 0.05), cos(uTime * 0.043)) * 0.04;
  vec2 bgUv = p * (0.5 + 0.02 * sin(uTime * 0.1)) + 0.5 + drift;
  vec3 bg = textureLod(uArt, clamp(bgUv, 0.0, 1.0), 7.0).rgb;
  float lum = dot(bg, vec3(0.299, 0.587, 0.114));
  bg = mix(vec3(lum), bg, 1.3) * 0.5;
  bg = mix(bg, uTint * 0.45, 0.3);
  vec2 toCover = p - vec2(0.0, 0.04);
  float halo = exp(-dot(toCover, toCover) * 5.5);
  bg += uTint * halo * (0.12 + 0.55 * uBeat + 0.3 * uLow);
  bg *= 1.0 - 0.7 * smoothstep(0.35, 1.1, length(p * vec2(0.8, 1.0)));

  // The cover: it breathes with the loudness and jumps out on a beat.
  float pop = mix(0.94, 1.0, uAppear);
  float half_ = 0.3 * pop * (1.0 + 0.05 * uRms + 0.065 * uKick + 0.015 * uBeat);
  vec2 c = toCover;
  float d = roundedBox(c, vec2(half_), 0.014);

  vec3 col = bg;
  // Reflection on a glossy surface below the cover.
  float below = -c.y - half_;
  if (below > 0.0) {
    vec2 rc = vec2(c.x, -c.y - 2.0 * half_);
    vec2 ruv = rc / (2.0 * half_) + 0.5;
    if (abs(rc.x) < half_ && ruv.y > 0.0) {
      float fade = (1.0 - smoothstep(0.0, 0.55, below / half_)) * 0.32 * uAppear;
      col = mix(col, texture(uArt, ruv).rgb * 0.85, fade);
    }
  }
  // A shadow under and around it.
  float shadow = exp(-max(d, 0.0) * 14.0) * 0.55;
  col *= 1.0 - shadow * smoothstep(-0.02, 0.0, d + 0.02) * 0.7;

  vec2 uv = c / (2.0 * half_) + 0.5;
  vec3 art = texture(uArt, clamp(uv, 0.0, 1.0)).rgb;
  // A band of light sweeping across the cover now and then.
  float sweep = fract(uTime * 0.07 + (uv.x + uv.y) * 0.35);
  art += vec3(0.08) * smoothstep(0.0, 0.05, sweep) * smoothstep(0.12, 0.05, sweep);
  art += uTint * 0.06 * uKick;
  float a = smoothstep(fwidth(d) * 1.2, -fwidth(d) * 1.2, d) * uAppear;
  // A fine light edge.
  float rim = smoothstep(0.004, 0.0, abs(d + 0.002)) * 0.25;
  col = mix(col, art + rim, a);
  o = vec4(col, 1.0);
}`;

/**
 * The cover, large, on a backdrop of itself blurred: breathing with the loudness, jumping out on
 * each beat, lit from behind in a colour taken from the cover, and reflected on glossy ground.
 */
export class AlbumStyle implements StyleRenderer {
  private readonly program: Program;

  constructor(private readonly host: StyleHost) {
    this.program = new Program(host.gl, FULLSCREEN_VS, FS);
  }

  resize(): void {}

  render(info: RenderInfo): void {
    const { gl, emptyVao, scene, artwork } = this.host;
    if (!artwork.texture) return;
    const { audio } = info;
    scene.bind();
    gl.bindVertexArray(emptyVao);
    this.program.use();
    bindTexture(gl, 0, artwork.texture);
    gl.uniform1i(this.program.u("uArt"), 0);
    gl.uniform2f(this.program.u("uRes"), info.width, info.height);
    gl.uniform3fv(this.program.u("uTint"), artwork.tint);
    gl.uniform1f(this.program.u("uTime"), info.time);
    gl.uniform1f(this.program.u("uRms"), audio.rms);
    gl.uniform1f(this.program.u("uBeat"), audio.beat);
    gl.uniform1f(this.program.u("uKick"), audio.kick);
    gl.uniform1f(this.program.u("uLow"), audio.low);
    const fade = Math.min(1, artwork.age / 0.8);
    gl.uniform1f(this.program.u("uAppear"), fade * fade * (3 - 2 * fade));
    gl.drawArrays(gl.TRIANGLES, 0, 3);
  }

  dispose(): void {
    this.program.dispose();
  }
}
