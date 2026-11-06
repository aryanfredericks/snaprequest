// GraphQL in the editor (loaded on demand: graphql-js is large): syntax, and with a schema,
// completion, validation and hover docs (cm6-graphql).
import { graphql } from "cm6-graphql";
import type { Extension } from "@codemirror/state";
import { buildClientSchema, type GraphQLSchema, type IntrospectionQuery } from "graphql";

export function graphqlLanguage(schema: GraphQLSchema | null): Extension {
  return graphql(schema ?? undefined);
}

/** A schema for the editor from an introspection `__schema` object; null when it does not build. */
export function clientSchema(introspection: unknown): GraphQLSchema | null {
  try {
    return buildClientSchema({ __schema: introspection } as IntrospectionQuery);
  } catch {
    return null;
  }
}
