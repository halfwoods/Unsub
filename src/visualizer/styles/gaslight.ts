import { GaslightField } from "../gaslight";
import { FULLSCREEN_VS, Program, VERSION, bindTexture, type StyleHost } from "../gl";
import type { RenderInfo, StyleRenderer } from "../styles";

const FRAGMENT = `${VERSION}
uniform sampler2D uField;
in vec2 vUv;
out vec4 o;
void main() { o = texture(uField, vUv); }
`;

/** The CPU field is shared with older WebKit's Canvas2D path; GL only scales and composites. */
export class GaslightStyle implements StyleRenderer {
  private readonly field = new GaslightField();
  private readonly program: Program;
  private readonly texture: WebGLTexture;

  constructor(private readonly host: StyleHost) {
    const { gl } = host;
    this.program = new Program(gl, FULLSCREEN_VS, FRAGMENT);
    const texture = gl.createTexture();
    if (!texture) { this.program.dispose(); throw new Error("could not create the Gaslight field"); }
    this.texture = texture;
    gl.bindTexture(gl.TEXTURE_2D, texture);
    gl.texStorage2D(gl.TEXTURE_2D, 1, gl.RGBA8, this.field.width, this.field.height);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
  }

  resize(): void { /* The field's memory and cost are independent of the window size. */ }

  render(info: RenderInfo): void {
    const { gl, emptyVao, scene } = this.host;
    const pixels = this.field.render(info.dt, info.audio, info.gaslight);
    scene.bind();
    gl.bindVertexArray(emptyVao);
    this.program.use();
    bindTexture(gl, 0, this.texture);
    gl.texSubImage2D(gl.TEXTURE_2D, 0, 0, 0, this.field.width, this.field.height, gl.RGBA, gl.UNSIGNED_BYTE, pixels);
    gl.uniform1i(this.program.u("uField"), 0);
    gl.drawArrays(gl.TRIANGLES, 0, 3);
  }

  dispose(): void {
    this.program.dispose();
    this.host.gl.deleteTexture(this.texture);
  }
}
