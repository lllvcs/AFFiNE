import { Logger } from '@nestjs/common';

import {
  BadRequest,
  CopilotQuotaExceeded,
  NetworkError,
} from '../../../base/error/errors.gen';

const LLM_TIMEOUT_ERROR_PREFIX = 'llm_timeout:';

const logger = new Logger('CopilotRuntime');

/**
 * Route failures reported by the native runtime.
 *
 * The native layer appends a diagnostic after the reason
 * (`no_compatible_target: slot=chat.default workspace=... profiles=0 ...`), so
 * only the leading reason token is matched here.
 */
const ROUTE_FAILURE_REASONS = [
  'byok_disabled',
  'no_compatible_target',
  'target_unavailable',
  'managed_preset_unavailable',
] as const;

type RouteFailureReason = (typeof ROUTE_FAILURE_REASONS)[number];

function routeFailureReason(message: string): RouteFailureReason | undefined {
  return ROUTE_FAILURE_REASONS.find(
    reason => message === reason || message.startsWith(`${reason}:`)
  );
}

function routeFailureSlot(message: string) {
  return /\bslot=([^\s,;]+)/.exec(message)?.[1];
}

function routeFailureMessage(reason: RouteFailureReason, message: string) {
  const slot = routeFailureSlot(message);
  switch (reason) {
    case 'byok_disabled':
      return 'Bring-your-own-key is disabled on this deployment (copilot.byok.enabled is false), so no AI model can be routed. Ask the administrator to enable it in the backend runtime configuration.';
    case 'target_unavailable':
      return 'The selected AI model is no longer available. Pick another model in the AI panel and try again.';
    case 'managed_preset_unavailable':
      return 'No managed AI provider is available for this feature on this deployment. Add a bring-your-own-key model in Settings → Workspace → AI.';
    case 'no_compatible_target':
    default:
      return (
        `No AI model is configured for this request${slot ? ` (${slot})` : ''}.` +
        ' Open Settings → Workspace → AI: add a bring-your-own-key model in the workspace you are chatting in, make sure it is enabled, and make sure its use cases cover what this request needs.' +
        ' AFFiNE AI sends tool definitions with every chat message, so chat needs both the "Chat" use case (text output) and the "Actions" use case (tool calling).'
      );
  }
}

function nativeErrorMessage(error: unknown) {
  if (error instanceof Error) {
    return error.message;
  }
  if (
    error &&
    typeof error === 'object' &&
    typeof (error as { message?: unknown }).message === 'string'
  ) {
    return (error as { message: string }).message;
  }
  return typeof error === 'string' ? error : undefined;
}

export function mapNativeSemanticError(error: unknown): unknown {
  const message = nativeErrorMessage(error);
  if (
    message === 'access_unavailable' ||
    message?.startsWith('access_unavailable:')
  ) {
    return new CopilotQuotaExceeded();
  }
  if (message?.startsWith(LLM_TIMEOUT_ERROR_PREFIX)) {
    return new NetworkError(
      message.slice(LLM_TIMEOUT_ERROR_PREFIX.length).trim() ||
        'LLM request timed out'
    );
  }
  if (message) {
    const reason = routeFailureReason(message);
    if (reason) {
      // The native diagnostic carries the workspace, the loaded profiles and each
      // model's capability match; keep it in the server log, the user only needs
      // the actionable part.
      logger.warn(`copilot route failed (${reason}): ${message}`);
      return new BadRequest(routeFailureMessage(reason, message));
    }
  }
  return error;
}
