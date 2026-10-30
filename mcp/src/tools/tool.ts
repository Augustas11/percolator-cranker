import type { z } from "zod";
import type { Drip } from "../drip";

export interface Tool<S extends z.ZodRawShape = z.ZodRawShape> {
  name: string;
  description: string;
  inputSchema?: S;
  run(drip: Drip, args: z.output<z.ZodObject<S>>): Promise<object>;
}

export const tool = <S extends z.ZodRawShape>(definition: Tool<S>): Tool<S> => definition;
