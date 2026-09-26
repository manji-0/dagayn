export * from "./a";
export * as bns from "./b";
export { fromB as renamedB, default as defB } from "./b";
export { default } from "./a";
import { fromA } from "./a";
export { fromA as localRenamed };
export type { Repo } from "../interfaces";
const local = 1;
export { local };
