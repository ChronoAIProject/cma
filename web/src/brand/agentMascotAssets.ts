import { failed } from '../assets/brand/pixel/motions/failed/assets';
import { idea } from '../assets/brand/pixel/motions/idea/assets';
import { love } from '../assets/brand/pixel/motions/love/assets';
import { preparing } from '../assets/brand/pixel/motions/preparing/assets';
import { question } from '../assets/brand/pixel/motions/question/assets';
import { ready } from '../assets/brand/pixel/motions/ready/assets';
import { rest } from '../assets/brand/pixel/motions/rest/assets';
import { working } from '../assets/brand/pixel/motions/working/assets';
import type { AgentVisualAction } from './agentVisual';

type MascotMotion =
  | typeof rest
  | typeof ready
  | typeof idea
  | typeof working
  | typeof question
  | typeof love
  | typeof preparing
  | typeof failed;

export const agentMascotAssets = {
  rest,
  ready,
  idea,
  working,
  question,
  love,
  preparing,
  failed,
} satisfies Record<AgentVisualAction, MascotMotion>;
