import { getHealth } from "@acme/api-client";

export async function boot(): Promise<string> {
  const health = await getHealth();
  return health.status;
}
