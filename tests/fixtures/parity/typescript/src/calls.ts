import DefaultShape, { AbstractShape as Abs, Box } from "./classes";
import * as fns from "./functions";
import { Outer } from "./namespaces";
import { UserService } from "./user.service";

export class Caller extends Box<object> {
  private svc = new UserService();
  private field = fns.arrow(1);
  constructor() {
    super({} as never);
    this.helper();
  }
  run(param = fns.decl(1)) {
    this.helper();
    super.helper();
    const s = new DefaultShape();
    s.area();
    fns.decl(2);
    fns.api.get("x");
    Outer.helper();
    this.svc?.getUser();
    maybe?.();
    obj?.a?.b();
    tag`hello ${1}`;
    const Ctor = Box;
    new Ctor({} as never);
    (fns.arrow)(3);
    fns["decl"](4);
    return Abs.name;
  }
  async later() {
    await import("./functions");
    const m = await import("./types");
    m.colorName(0);
  }
}
declare const maybe: (() => void) | undefined;
declare const obj: any;
declare function tag(s: TemplateStringsArray, ...v: unknown[]): string;
