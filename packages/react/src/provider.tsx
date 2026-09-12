import { createContext, useContext, useEffect, useRef, type ReactNode } from "react";
import type { BrowserClient } from "@browserkit/core";

const BrowserClientContext = createContext<BrowserClient | null>(null);

export interface BrowserProviderProps {
  client: BrowserClient;
  children: ReactNode;
}

export function BrowserProvider({ client, children }: BrowserProviderProps) {
  const generation = useRef(0);

  useEffect(() => {
    const current = ++generation.current;
    void client.connect().catch((error) => {
      if (generation.current === current) console.error("BrowserKit connection failed", error);
    });
    return () => {
      queueMicrotask(() => {
        if (generation.current === current) client.disconnect();
      });
    };
  }, [client]);

  return <BrowserClientContext.Provider value={client}>{children}</BrowserClientContext.Provider>;
}

export function useBrowser(): BrowserClient {
  const client = useContext(BrowserClientContext);
  if (!client) throw new Error("useBrowser must be used inside BrowserProvider");
  return client;
}
