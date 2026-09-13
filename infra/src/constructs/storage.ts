
import { Construct } from "constructs";
import type { StageConfig } from "../config";
import { ssmValue } from "../config";

export interface StorageProps {
  readonly config: StageConfig;
}

export interface PostgresConnectionSettings {
  readonly host: string;
  readonly port: string;
  readonly database: string;
  readonly username: string;
  readonly password: string;
  readonly maxConnections: string;
    readonly sslMode: "verify-full" | "disable";
    readonly sslRootCert: string | undefined;
}

export class Storage extends Construct {
  readonly postgres: PostgresConnectionSettings;

  constructor(scope: Construct, id: string, props: StorageProps) {
    super(scope, id);

    this.postgres = postgresConnectionSettings(props.config);
  }
}

function postgresConnectionSettings(config: StageConfig): PostgresConnectionSettings {
  if (config.isEphemeral) {
    return {
      host: "host.docker.internal",
      port: "5432",
      database: "postgres",
      username: "postgres",
      password: "postgres",
      maxConnections: "2",
      sslMode: "disable",
      sslRootCert: undefined,
    };
  }

  return {
    host: ssmValue(`/postgres/${config.stage}/host`),
    port: ssmValue(`/postgres/${config.stage}/port`),
    database: ssmValue(`/postgres/${config.stage}/database`),
    username: ssmValue(`/postgres/${config.stage}/username`),
    password: ssmValue(`/secrets/${config.stage}/postgres-password`),
    maxConnections: "2",
    sslMode: "verify-full",
    sslRootCert: ssmValue(`/postgres/${config.stage}/ssl-root-cert-path`),
  };
}
