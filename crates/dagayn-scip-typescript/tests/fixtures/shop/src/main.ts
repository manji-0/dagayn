import leftPad from "left-pad";
import { Cart, describe, discount, format, total } from "./cart";

const cart = new Cart();
cart.add(3);
const label = "🛒"; total([1, 2]);
export const out = [leftPad(format(cart.sum()), 8), discount(5), describe([1])];
export const doubled = [1, 2].map((item) => item * 2);
