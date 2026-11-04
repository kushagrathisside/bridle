//! Combining the results of several search providers: duplicates are merged by canonical address, the order comes from
//! reciprocal rank fusion (the scores of unrelated providers are not comparable, their ranks are), and no one site fills the list.

use std::collections::BTreeMap;

use super::providers::SearchHit;

/// The constant of reciprocal rank fusion. 60 is the value from the original paper and the usual default.
const RRF_K: f64 = 60.0;

#[derive(Debug, Clone, PartialEq)]
pub struct Fused {
    pub hit: SearchHit,
    pub score: f64,
    /// Every provider that returned it.
    pub providers: Vec<String>,
}

/// The address with the parts that do not name a page removed: the fragment, tracking parameters, a trailing slash, the case
/// of the scheme and host, and a leading `www.`.
pub fn canonical_url(url: &str) -> String {
    let url = url.trim().split('#').next().unwrap_or("");
    let (head, query) = url
        .split_once('?')
        .map_or((url, None), |(h, q)| (h, Some(q)));
    let (scheme, rest) = head.split_once("://").unwrap_or(("https", head));
    let (host, path) = rest.split_once('/').map_or((rest, ""), |(h, p)| (h, p));
    let host = host.to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host);
    let mut out = format!(
        "{}://{host}/{}",
        scheme.to_ascii_lowercase(),
        path.trim_end_matches('/')
    );
    if let Some(q) = query {
        let kept: Vec<&str> = q
            .split('&')
            .filter(|p| {
                !p.is_empty()
                    && !p.starts_with("utm_")
                    && !p.starts_with("fbclid")
                    && !p.starts_with("gclid")
                    && !p.starts_with("ref=")
            })
            .collect();
        if !kept.is_empty() {
            out.push('?');
            out.push_str(&kept.join("&"));
        }
    }
    out.trim_end_matches('/').to_owned()
}

/// The site a host belongs to, approximately: its last two labels (three when the second to last is a short generic one such as
/// `co`, as in `bbc.co.uk`). A list of public suffixes would be exact; this is enough to stop one site filling the results.
pub fn registered_domain(host: &str) -> String {
    let labels: Vec<&str> = host.trim_start_matches("www.").split('.').collect();
    let n = labels.len();
    if n <= 2 {
        return labels.join(".");
    }
    let take = if matches!(
        labels[n - 2],
        "co" | "com" | "org" | "net" | "gov" | "ac" | "edu"
    ) && labels[n - 1].len() == 2
    {
        3
    } else {
        2
    };
    labels[n - take..].join(".")
}

fn host_of(url: &str) -> String {
    url.split_once("://")
        .map_or(url, |(_, r)| r)
        .split(['/', '?', ':'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// Fuses ranked lists (one per provider, best first) into at most `limit` results, at most `per_domain` from one site (the cap is
/// relaxed only when there would otherwise be fewer than `limit` results).
pub fn fuse(lists: &[Vec<SearchHit>], limit: usize, per_domain: usize) -> Vec<Fused> {
    let mut by_url: BTreeMap<String, Fused> = BTreeMap::new();
    for list in lists {
        for (index, hit) in list.iter().enumerate() {
            let key = canonical_url(&hit.url);
            let gain = 1.0 / (RRF_K + (index as f64) + 1.0);
            let entry = by_url.entry(key).or_insert_with(|| Fused {
                hit: hit.clone(),
                score: 0.0,
                providers: Vec::new(),
            });
            entry.score += gain;
            if !entry.providers.contains(&hit.provider) {
                entry.providers.push(hit.provider.clone());
            }
            if hit.snippet.len() > entry.hit.snippet.len() {
                entry.hit.snippet = hit.snippet.clone();
            }
            if entry.hit.published.is_none() {
                entry.hit.published = hit.published.clone();
            }
        }
    }
    let mut all: Vec<Fused> = by_url.into_values().collect();
    all.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.hit.url.cmp(&b.hit.url))
    });
    let mut kept: Vec<Fused> = Vec::new();
    let mut skipped: Vec<Fused> = Vec::new();
    let mut per_site: BTreeMap<String, usize> = BTreeMap::new();
    for item in all {
        let site = registered_domain(&host_of(&item.hit.url));
        let count = per_site.entry(site).or_insert(0);
        if *count < per_domain {
            *count += 1;
            kept.push(item);
        } else {
            skipped.push(item);
        }
    }
    for item in skipped {
        if kept.len() >= limit {
            break;
        }
        kept.push(item);
    }
    kept.truncate(limit);
    kept
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn hit(provider: &str, url: &str, snippet: &str) -> SearchHit {
        SearchHit {
            title: url.to_owned(),
            url: url.to_owned(),
            snippet: snippet.to_owned(),
            provider: provider.to_owned(),
            published: None,
        }
    }

    #[test]
    fn the_same_page_in_different_spellings_has_one_canonical_address() {
        let a = canonical_url("HTTPS://www.Example.org/a/b/?utm_source=x&id=7#top");
        assert_eq!(a, "https://example.org/a/b?id=7");
        assert_eq!(
            canonical_url("https://example.org/a/b"),
            a.replace("?id=7", "")
        );
        assert_eq!(canonical_url("https://example.org/"), "https://example.org");
    }

    #[test]
    fn sites_are_told_apart_by_their_registered_domain() {
        assert_eq!(registered_domain("docs.rs"), "docs.rs");
        assert_eq!(registered_domain("blog.example.org"), "example.org");
        assert_eq!(registered_domain("www.bbc.co.uk"), "bbc.co.uk");
        assert_eq!(registered_domain("a.b.c.example.com"), "example.com");
    }

    #[test]
    fn a_result_both_providers_return_ranks_above_one_only_one_returned() {
        let a = vec![
            hit("brave", "https://one.org/x", "short"),
            hit("brave", "https://two.org/y", ""),
        ];
        let b = vec![
            hit(
                "searxng",
                "https://two.org/y?utm_medium=z",
                "a longer snippet",
            ),
            hit("searxng", "https://three.org/z", ""),
        ];
        let fused = fuse(&[a, b], 10, 3);
        assert_eq!(
            fused[0].hit.url, "https://two.org/y",
            "found by both: first, even though it was second in each list"
        );
        assert_eq!(fused[0].providers, ["brave", "searxng"]);
        assert_eq!(fused[0].hit.snippet, "a longer snippet");
        assert_eq!(fused.len(), 3);
    }

    #[test]
    fn one_site_does_not_fill_the_list_unless_nothing_else_is_left() {
        let many: Vec<SearchHit> = (0..6)
            .map(|i| hit("p", &format!("https://spam.example.com/{i}"), ""))
            .collect();
        let other = vec![
            hit("p", "https://other.org/a", ""),
            hit("p", "https://third.net/b", ""),
        ];
        let fused = fuse(&[many.clone(), other], 4, 2);
        let sites: Vec<String> = fused
            .iter()
            .map(|f| registered_domain(&host_of(&f.hit.url)))
            .collect();
        assert_eq!(
            sites.iter().filter(|s| *s == "example.com").count(),
            2,
            "{sites:?}"
        );
        assert_eq!(fused.len(), 4);
        // With nothing else, the cap is relaxed so the caller still gets its results.
        assert_eq!(fuse(&[many], 5, 2).len(), 5);
    }

    #[test]
    fn the_order_is_stable_and_the_limit_is_respected() {
        let list: Vec<SearchHit> = (0..20)
            .map(|i| hit("p", &format!("https://s{i}.org/"), ""))
            .collect();
        let fused = fuse(std::slice::from_ref(&list), 5, 3);
        assert_eq!(fused.len(), 5);
        assert_eq!(fused[0].hit.url, "https://s0.org/");
        assert_eq!(fuse(std::slice::from_ref(&list), 5, 3), fused);
    }
}
