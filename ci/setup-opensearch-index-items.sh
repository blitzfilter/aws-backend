#!/usr/bin/env bash
# script to be called from repository root dir
set -euo pipefail

INDEX_NAME="items"
MAPPING_FILE="opensearch/mappings/items.json"
STACK_NAME="staging-${ENV_SUFFIX}"

# Resolve OpenSearch domain name + endpoint from CloudFormation Outputs
DOMAIN_NAME=$(aws cloudformation describe-stacks \
  --stack-name "$STACK_NAME" \
  --query "Stacks[0].Outputs[?OutputKey=='OpenSearchDomainName'].OutputValue" \
  --output text)

RAW_ENDPOINT=$(aws cloudformation describe-stacks \
  --stack-name "$STACK_NAME" \
  --query "Stacks[0].Outputs[?OutputKey=='OpenSearchDomainEndpoint'].OutputValue" \
  --output text)

if [ -z "$DOMAIN_NAME" ]; then
  echo "❌ Could not resolve OpenSearch domain-name from stack: $STACK_NAME"
  exit 1
fi

if [ -z "$RAW_ENDPOINT" ]; then
  echo "❌ Could not resolve OpenSearch endpoint from stack: $STACK_NAME"
  exit 1
fi

# Strip protocol if included
ENDPOINT=${RAW_ENDPOINT#https://}
echo "✅ Using OpenSearch endpoint: $ENDPOINT"

# Wait until the domain is ACTIVE
echo "⏳ Waiting for OpenSearch domain $DOMAIN_NAME to become ACTIVE..."

while true; do
  PROCESSING=$(aws opensearch describe-domain --domain-name "$DOMAIN_NAME" \
    --query "DomainStatus.Processing" --output text)

  if [ "$PROCESSING" == "False" ]; then
    echo "✅ Domain $DOMAIN_NAME is ACTIVE."
    break
  else
    echo "⏳ Domain still processing... waiting 15s"
    sleep 15
  fi
done

# Delete index if it exists (check with signed GET)
STATUS=$(aws opensearch \
  --region "$REGION" \
  --endpoint https://"$ENDPOINT" \
  es-http-head \
  --path "/$INDEX_NAME" \
  --output text \
  --query "ResponseMetadata.HTTPStatusCode" 2>/dev/null || echo "404")

if [ "$STATUS" -eq 200 ]; then
  echo "🔄 Deleting existing index $INDEX_NAME..."
  aws opensearch \
    --region "$REGION" \
    --endpoint https://"$ENDPOINT" \
    es-http-delete \
    --path "/$INDEX_NAME"
else
  echo "ℹ️ Index $INDEX_NAME does not exist, skipping delete."
fi

# Recreate index with mapping
echo "📦 Creating index $INDEX_NAME with mapping from $MAPPING_FILE..."
aws opensearch \
  --region "${AWS_REGION}" \
  --endpoint https://"$ENDPOINT" \
  es-http-put \
  --path "/$INDEX_NAME" \
  --body "file://$MAPPING_FILE"

echo "🎉 Index $INDEX_NAME successfully created."
