export function trendingKeywords(rule: string): string[] {
  return Array.from(
    new Set(rule.trim().toLowerCase().split(/\s+/u).filter(Boolean)),
  )
}

export function matchesTrendingRule(query: string, rule: string): boolean {
  const normalizedQuery = query.toLowerCase()
  const keywords = trendingKeywords(rule)
  return (
    keywords.length > 0 &&
    keywords.every((keyword) => normalizedQuery.includes(keyword))
  )
}
