import ava from 'ava';

import {
  BadRequest,
  CopilotQuotaExceeded,
  NetworkError,
} from '../../../../base/error/errors.gen';
import { mapNativeSemanticError } from '../native-errors';

const nativeDetail =
  'no_compatible_target: slot=chat.default workspace=a36be95a-0000-0000-0000-000000000000 deployment=selfhosted copilot.byok.enabled=true serverByok=true localByok=false profiles=0 (no BYOK profile was loaded for this workspace)';

ava('bare no_compatible_target becomes an actionable bad request', t => {
  const mapped = mapNativeSemanticError(new Error('no_compatible_target'));
  t.true(mapped instanceof BadRequest);
  const message = (mapped as BadRequest).message;
  t.regex(message, /No AI model is configured for this request/);
  t.regex(message, /Settings → Workspace → AI/);
  t.regex(message, /"Chat" use case \(text output\)/);
  // Chat requests always carry tool definitions, so the route demands tool calling
  // on top of text output - say so instead of sending users on a wild goose chase.
  t.regex(message, /"Actions" use case \(tool calling\)/);
});

ava('the native route diagnostic stays out of the user facing message', t => {
  const mapped = mapNativeSemanticError(new Error(nativeDetail));
  t.true(mapped instanceof BadRequest);
  const message = (mapped as BadRequest).message;
  t.false(message.includes('profiles='));
  t.false(message.includes('a36be95a'));
  t.regex(message, /\(chat\.default\)/);
});

ava('other route failures keep their own guidance', t => {
  const byokDisabled = mapNativeSemanticError(
    new Error('byok_disabled: slot=chat.default workspace=ws profiles=0')
  );
  t.true(byokDisabled instanceof BadRequest);
  t.regex((byokDisabled as BadRequest).message, /Bring-your-own-key/);

  const target = mapNativeSemanticError(
    new Error('target_unavailable: slot=chat.default workspace=ws profiles=1')
  );
  t.true(target instanceof BadRequest);
  t.regex((target as BadRequest).message, /no longer available/);

  const managed = mapNativeSemanticError(
    new Error('managed_preset_unavailable')
  );
  t.true(managed instanceof BadRequest);
  t.regex((managed as BadRequest).message, /managed AI provider/);
});

ava('access_unavailable still maps to the quota error, bare or enriched', t => {
  t.true(
    mapNativeSemanticError(new Error('access_unavailable')) instanceof
      CopilotQuotaExceeded
  );
  t.true(
    mapNativeSemanticError(
      new Error('access_unavailable: slot=chat.default profiles=0')
    ) instanceof CopilotQuotaExceeded
  );
});

ava('timeouts and unknown errors keep their existing behaviour', t => {
  const timeout = mapNativeSemanticError(new Error('llm_timeout: 30s'));
  t.true(timeout instanceof NetworkError);
  t.is((timeout as NetworkError).message, '30s');

  const unknown = new Error('something else');
  t.is(mapNativeSemanticError(unknown), unknown);
  t.is(mapNativeSemanticError(undefined), undefined);
});
