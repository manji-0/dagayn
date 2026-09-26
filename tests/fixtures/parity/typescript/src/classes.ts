import { Repo, Logger } from "./interfaces";
import { Base, Mixin } from "./lib/base";

export abstract class AbstractShape {
  abstract area(): number;
  abstract get label(): string;
  describe(): string {
    return this.label + this.area();
  }
}

abstract class B {}

export default class DefaultShape extends AbstractShape implements Repo, Logger {
  area(): number { return 1; }
  get label(): string { return "d"; }
  find(id: string): string { return id; }
  log(msg: string): void {}
}

export class Box<T extends object> extends Mixin(Base) {
  #secret = 1;
  static count = 0;
  readonly name: string = "box";
  declare hidden: number;
  accessor size = 2;
  private handler = () => this.helper();
  [key: string]: unknown;

  constructor(private readonly repo: Repo, public logger?: Logger) {
    super(repo);
    Box.count++;
  }

  static create(): Box<object> { return new Box({} as Repo); }
  async load(): Promise<void> { await this.repo.find("x"); }
  *items(): Generator<number> { yield 1; }
  get value(): number { return this.#secret; }
  set value(v: number) { this.#secret = v; }
  #privateMethod(): void {}
  ["computed"](): void {}
  helper(): void { super.toString(); }

  overloaded(a: string): string;
  overloaded(a: number): number;
  overloaded(a: any): any { return a; }
}

export const Anon = class {
  run() {}
};

export const Named = class InnerName {
  go() {}
};

@sealed
@Injectable({ providedIn: "root" })
export class Decorated {
  @Input() title = "";
  @HostListener("click", ["$event"])
  onClick(@Inject(TOKEN) e: Event) {}
}

function sealed(ctor: Function) {}
declare function Injectable(o: object): ClassDecorator;
declare function Input(): PropertyDecorator;
declare function HostListener(a: string, b: string[]): MethodDecorator;
declare function Inject(t: unknown): ParameterDecorator;
declare const TOKEN: unique symbol;
