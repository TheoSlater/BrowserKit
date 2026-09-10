import { createContext, useContext, type ReactNode } from "react";
import type { BrowserClient } from "@browserkit/core";

const BrowserContext = createContext<BrowserClient | null>(null);

export function BrowserProvider({ client, children }: { client: BrowserClient; children: ReactNode }) {
  return <BrowserContext.Provider value={client}>{children}</BrowserContext.Provider>;
}

export function useBrowser(): BrowserClient {
  const client = useContext(BrowserContext);
  if (!client) throw new Error("BrowserProvider is required");
  return client;
}
