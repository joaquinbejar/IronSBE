/*
 * C shim wrapping inline ibverbs functions as real symbols.
 *
 * ibv_post_send, ibv_post_recv, and ibv_poll_cq are static inline
 * wrappers in infiniband/verbs.h that dispatch through function
 * pointers on the QP/CQ context.  bindgen cannot generate Rust
 * bindings for them, so we wrap them here.
 */

#include <infiniband/verbs.h>

int ironsbe_ibv_post_send(struct ibv_qp *qp,
                           struct ibv_send_wr *wr,
                           struct ibv_send_wr **bad_wr)
{
    return ibv_post_send(qp, wr, bad_wr);
}

int ironsbe_ibv_post_recv(struct ibv_qp *qp,
                           struct ibv_recv_wr *wr,
                           struct ibv_recv_wr **bad_wr)
{
    return ibv_post_recv(qp, wr, bad_wr);
}

int ironsbe_ibv_poll_cq(struct ibv_cq *cq, int num_entries,
                         struct ibv_wc *wc)
{
    return ibv_poll_cq(cq, num_entries, wc);
}
