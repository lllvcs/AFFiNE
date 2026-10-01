import serverNativeModule from '@affine/server-native';
import { z } from 'zod';

import { defineNativeModuleConfig } from '../../base';

export interface AuthConfig {
  session: {
    ttl: number;
    ttr: number;
  };
  token: {
    accessTokenTtl: number;
    refreshIdleTtl: number;
    refreshAbsoluteTtl: number;
    refreshGracePeriod: number;
    refreshRetention: number;
  };
  allowSignup: boolean;
  allowSignupForOauth: boolean;
  requireEmailDomainVerification: boolean;
  /**
   * Which sign-in methods this deployment accepts. All default to enabled; a
   * "sign in with OIDC only" instance disables password and magicLink. Fields
   * may be omitted in config.json, so they are optional here too.
   */
  signInMethods: ConfigItem<{
    password?: boolean;
    magicLink?: boolean;
    oauth?: boolean;
  }>;
  newAccountActionDelay: number;
  trustedCloudflareHeaders: boolean;
  signInRateLimit: ConfigItem<{
    ttl: number;
    ipLimit: number;
    emailLimit: number;
  }>;
  passwordRequirements: ConfigItem<{
    min: number;
    max: number;
  }>;
}

declare global {
  interface AppConfigSchema {
    auth: AuthConfig;
  }
}

defineNativeModuleConfig(
  'auth',
  serverNativeModule.appConfigDescriptors('auth'),
  serverNativeModule.validateAppConfigValue,
  {
    signInMethods: {
      desc: 'Which sign-in methods are accepted. Disable password and magic link to run an OIDC-only instance.',
      default: { password: true, magicLink: true, oauth: true },
      // The fields are optional so a partial object in config.json
      // ({"password": false}) validates like the native schema, which fills the
      // missing fields from Default; AuthService.signInMethods normalises them
      // back to a full object.
      shape: z
        .object({
          password: z.boolean().optional(),
          magicLink: z.boolean().optional(),
          oauth: z.boolean().optional(),
        })
        .strict(),
    },
    trustedCloudflareHeaders: {
      desc: 'Whether request abuse source facts should trust Cloudflare headers from the origin edge.',
      default: false,
      shape: z.boolean(),
    },
    signInRateLimit: {
      desc: 'Limits for sign-in attempts shared through Redis by source IP and email. ttl is measured in milliseconds.',
      default: {
        ttl: 60_000,
        ipLimit: 20,
        emailLimit: 5,
      },
      shape: z
        .object({
          ttl: z.number().int().positive(),
          ipLimit: z.number().int().positive(),
          emailLimit: z.number().int().positive(),
        })
        .strict(),
    },
    passwordRequirements: {
      desc: 'The password strength requirements when set new password.',
      default: {
        min: 8,
        max: 32,
      },
      shape: z
        .object({
          min: z.number().min(1),
          max: z.number().max(100),
        })
        .strict()
        .refine(data => data.min < data.max, {
          message:
            'Minimum length of password must be less than maximum length',
        }),
      schema: {
        type: 'object',
        properties: {
          min: { type: 'number' },
          max: { type: 'number' },
        },
      },
    },
  }
);
