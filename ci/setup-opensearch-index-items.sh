#!/usr/bin/env bash
# script to be called from repository root dir
set -euo pipefail

INDEX_NAME="items"
MAPPING_FILE="opensearch/mappings/items.json"
STACK_NAME="staging-${ENV_SUFFIX}"

# Resolve OpenSearch endpoint from CloudFormation Outputs
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

# Delete index if it exists
STATUS=$(curl -s -o /dev/null -w "%{http_code}" "https://$ENDPOINT/$INDEX_NAME")
if [ "$STATUS" -eq 200 ]; then
  echo "🔄 Deleting existing index $INDEX_NAME..."
  curl -s -X DELETE "https://$ENDPOINT/$INDEX_NAME"
else
  echo "ℹ️ Index $INDEX_NAME does not exist, skipping delete."
fi

# Recreate index with mapping
echo "📦 Creating index $INDEX_NAME with mapping from $MAPPING_FILE..."
curl -s -X PUT "https://$ENDPOINT/$INDEX_NAME" \
  -H 'Content-Type: application/json' \
  -d @"$MAPPING_FILE"

echo "🎉 Index $INDEX_NAME successfully created."
