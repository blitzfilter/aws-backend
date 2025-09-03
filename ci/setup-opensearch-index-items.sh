#!/usr/bin/env bash
# script to be called from repository root dir
set -euo pipefail

INDEX_NAME="items"
MAPPING_FILE="opensearch/mappings/items.json"

# Resolve OpenSearch domain name + endpoint from CloudFormation Outputs
DOMAIN_NAME=$(aws cloudformation describe-stacks \
  --stack-name "$STACK_NAME" \
  --query "Stacks[0].Outputs[?OutputKey=='OpensearchDomainName'].OutputValue" \
  --output text)

RAW_ENDPOINT=$(aws cloudformation describe-stacks \
  --stack-name "$STACK_NAME" \
  --query "Stacks[0].Outputs[?OutputKey=='OpensearchItemDomainEndpointUrl'].OutputValue" \
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

# Configure CLI profile
echo -e "\n es" | opensearch-cli profile create --name "ci" \
  --endpoint "$RAW_ENDPOINT" \
  --auth-type "aws-iam"

# Create index if not exists
if opensearch-cli curl get --path "$INDEX_NAME" --profile ci | jq -e '.[].status? // empty' >/dev/null 2>&1; then
   echo "Index $INDEX_NAME exists already, skipping creation."
else
  echo "📦 Creating index with mapping from $MAPPING_FILE..."
  opensearch-cli curl put \
    --path "$INDEX_NAME" \
    --data "@$MAPPING_FILE" \
    --profile ci
fi

# Configure refresh-interval for index
echo "Configuring refresh-interval for index $INDEX_NAME..."
opensearch-cli curl put \
  --path "$INDEX_NAME/_settings" \
  --data '{
    "index": {
      "refresh_interval": "5m"
    }
  }'
