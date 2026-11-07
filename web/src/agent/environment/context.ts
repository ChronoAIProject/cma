import { createContext } from 'react';
import type { AgentComputerObservation } from './observation';

export const ComputerContext = createContext<AgentComputerObservation | undefined>(undefined);
