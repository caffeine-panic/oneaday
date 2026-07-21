import type { AdapterId, AuthenticationMode } from "./registry";

export const authLabels: Record<AuthenticationMode, string> = {
  none: "无认证",
  usernamePassword: "用户名 / 密码",
  digest: "Digest",
  custom: "自定义上下文",
  mseAccessKey: "阿里云 MSE AccessKey",
};

export function authModes(adapter: AdapterId): AuthenticationMode[] {
  if (adapter === "zookeeper") return ["none", "digest"];
  if (adapter === "nacos")
    return ["none", "usernamePassword", "mseAccessKey", "custom"];
  return ["none", "usernamePassword"];
}

export function credentialIdentityLabel(mode: AuthenticationMode): string {
  return mode === "mseAccessKey" ? "AccessKey ID" : "用户名";
}

export function credentialSecretLabel(mode: AuthenticationMode): string {
  if (mode === "digest") return "Digest 密码";
  return mode === "mseAccessKey" ? "AccessKey Secret" : "密码";
}
