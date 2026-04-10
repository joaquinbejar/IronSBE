/* Wrapper header that pulls in the DPDK types bindgen needs.
 *
 * We only include the headers whose types we actually use in Rust.
 * bindgen's allowlist further limits the output to the specific
 * functions/types we reference. */

#include <rte_eal.h>
#include <rte_ethdev.h>
#include <rte_mbuf.h>
#include <rte_mempool.h>
