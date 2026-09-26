/** @typedef {{a: number}} Foo */
export class JsClass {
  #priv = 1;
  static s = 2;
  field = function () { return 1; };
  get g() { return 1; }
}
export const obj = { m() {}, n: () => {} };
