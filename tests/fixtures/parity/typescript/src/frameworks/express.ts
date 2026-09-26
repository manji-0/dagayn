import express from "express";
import { decl } from "../functions";
const app = express();
const router = express.Router();
app.get("/users", (req, res) => { res.json(decl(1)); });
router.post("/items", async function createItem(req, res) { decl(2); });
app.use(authMiddleware);
function authMiddleware(req: any, res: any, next: any) { next(); }
export async function handler(event: unknown) { return decl(3); }
export const lambdaHandler = async (event: unknown) => decl(4);
app.listen(3000);
