import { decl } from "../../../../../functions";
export async function GET(request: Request) { return Response.json(decl(1)); }
export const POST = async (request: Request) => Response.json({});
