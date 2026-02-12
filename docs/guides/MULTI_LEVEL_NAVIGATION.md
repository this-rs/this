# Multi-Level Navigation Guide

## 🎯 Overview

this-rs supports **multi-level entity relationships** through semantic URLs, allowing you to navigate complex entity graphs naturally.

## 🌳 Entity Relationship Examples

### Example 1: Order → Invoice → Payment

```
Order
  └─► Invoice
        └─► Payment
```

**Configuration**:
```yaml
links:
  - link_type: has_invoice
    source_type: order
    target_type: invoice
    forward_route_name: invoices
    reverse_route_name: order
    
  - link_type: has_payment
    source_type: invoice
    target_type: payment
    forward_route_name: payments
    reverse_route_name: invoice
```

**Generated Routes**:
```bash
# Level 1: Order → Invoice
GET /orders/{order_id}/invoices
GET /orders/{order_id}/invoices/{invoice_id}

# Level 2: Invoice → Payment  
GET /invoices/{invoice_id}/payments
GET /invoices/{invoice_id}/payments/{payment_id}

# Reverse: Payment → Invoice → Order
GET /payments/{payment_id}/invoice
GET /invoices/{invoice_id}/order
```

---

## 🔄 Navigation Patterns

### Pattern 1: Forward Navigation (Source → Target)

Navigate from parent to children:

```bash
# Get all invoices for an order
GET /orders/abc-123/invoices

# Get all payments for an invoice
GET /invoices/inv-456/payments
```

**Response includes target entities**:
```json
{
  "links": [
    {
      "target_id": "inv-456",
      "target": {
        "id": "inv-456",
        "amount": 1500.00,
        // Full invoice data
      }
    }
  ]
}
```

### Pattern 2: Reverse Navigation (Target → Source)

Navigate from child back to parent:

```bash
# Get the order for an invoice
GET /invoices/inv-456/order

# Get the invoice for a payment
GET /payments/pay-789/invoice
```

**Response includes source entities**:
```json
{
  "links": [
    {
      "source_id": "abc-123",
      "source": {
        "id": "abc-123",
        "number": "ORD-123",
        // Full order data
      }
    }
  ]
}
```

### Pattern 3: Multi-Hop Navigation

Navigate multiple levels:

```bash
# Step 1: Get order
GET /orders/abc-123

# Step 2: Get invoices for order
GET /orders/abc-123/invoices
# Returns: [{ "target": { "id": "inv-456", ... } }]

# Step 3: Get payments for invoice
GET /invoices/inv-456/payments  
# Returns: [{ "target": { "id": "pay-789", ... } }]

# Or reverse: payment → invoice → order
GET /payments/pay-789/invoice
GET /invoices/inv-456/order
```

---

## 🎨 Complex Relationship Examples

### Example 2: Multiple Link Types

```
User
  ├─(owner)─► Car
  └─(driver)─► Car
```

**Configuration**:
```yaml
links:
  - link_type: owner
    source_type: user
    target_type: car
    forward_route_name: cars-owned
    reverse_route_name: owner
    
  - link_type: driver
    source_type: user
    target_type: car
    forward_route_name: cars-driven
    reverse_route_name: drivers
```

**Routes**:
```bash
# Cars owned by user
GET /users/123/cars-owned

# Cars driven by user  
GET /users/123/cars-driven

# Owner of car
GET /cars/456/owner

# Drivers of car
GET /cars/456/drivers
```

### Example 3: Many-to-Many Relationships

```
Project
  └─(has_member)─► User
                    └─(has_skill)─► Skill
```

**Configuration**:
```yaml
links:
  - link_type: has_member
    source_type: project
    target_type: user
    forward_route_name: members
    reverse_route_name: projects
    
  - link_type: has_skill
    source_type: user
    target_type: skill
    forward_route_name: skills
    reverse_route_name: users
```

**Navigation**:
```bash
# Get all members of a project
GET /projects/proj-1/members

# Get all projects for a user
GET /users/user-1/projects

# Get all skills for a user
GET /users/user-1/skills

# Get all users with a skill
GET /skills/skill-1/users
```

---

## 🔗 Creating Links at Multiple Levels

### Method 1: Link Existing Entities

```bash
# Link order → invoice
POST /orders/abc-123/invoices/inv-456
Body: { "metadata": { "created_by": "system" } }

# Link invoice → payment
POST /invoices/inv-456/payments/pay-789
Body: { "metadata": { "method": "credit_card" } }
```

### Method 2: Create Entity + Link Automatically

```bash
# Create new invoice and link to order
POST /orders/abc-123/invoices
Body: {
  "entity": {
    "number": "INV-999",
    "amount": 2000.00,
    "status": "pending"
  },
  "metadata": {
    "created_by": "api"
  }
}

# Then create payment and link to invoice
POST /invoices/{new_invoice_id}/payments
Body: {
  "entity": {
    "amount": 2000.00,
    "method": "wire_transfer"
  }
}
```

---

## 📊 Querying Strategies

### Strategy 1: Top-Down (Parent → Children)

Start from the root entity and navigate down:

```bash
# 1. Get order
GET /orders/abc-123

# 2. Get its invoices (enriched)
GET /orders/abc-123/invoices

# 3. For each invoice, get payments
GET /invoices/inv-1/payments
GET /invoices/inv-2/payments
```

### Strategy 2: Bottom-Up (Child → Parents)

Start from a leaf entity and navigate up:

```bash
# 1. Get payment
GET /payments/pay-789

# 2. Get its invoice (enriched)
GET /payments/pay-789/invoice

# 3. Get the order (enriched)
GET /invoices/inv-456/order
```

### Strategy 3: Breadth-First

Get all entities at one level before moving to the next:

```bash
# Level 1: All orders
GET /orders

# Level 2: All invoices for each order
for each order:
  GET /orders/{order_id}/invoices

# Level 3: All payments for each invoice
for each invoice:
  GET /invoices/{invoice_id}/payments
```

---

## 💡 Best Practices

### 1. Design Clear Hierarchies

```yaml
# ✅ Good: Clear parent-child relationships
Order → Invoice → Payment
User → Post → Comment

# ❌ Avoid: Circular dependencies
A → B → C → A
```

### 2. Use Semantic Route Names

```yaml
# ✅ Good: Descriptive names
forward_route_name: invoices      # /orders/123/invoices
forward_route_name: members       # /projects/456/members

# ❌ Avoid: Generic names
forward_route_name: links         # /orders/123/links (unclear)
forward_route_name: relations     # /projects/456/relations (unclear)
```

### 3. Leverage Auto-Enrichment

```bash
# ✅ Good: Use enriched responses
GET /orders/123/invoices
# Returns invoices with full data in one request

# ❌ Avoid: Multiple separate queries
GET /orders/123/invoices  # Get link IDs
GET /invoices/id1         # Then fetch each invoice
GET /invoices/id2
GET /invoices/id3
```

### 4. Index for Performance

```rust
// ✅ Good: Index frequently queried fields
impl_data_entity!(Order, "order", ["number", "customer_name"], {
    number: String,
    customer_name: String,
});
```

---

## 🎯 Real-World Example

### E-Commerce Order Fulfillment

```
Customer
  └─► Order
        ├─► OrderItem (product, quantity)
        ├─► Invoice
        │     └─► Payment
        └─► Shipment
              └─► TrackingEvent
```

**Configuration**:
```yaml
links:
  - { link_type: has_order, source_type: customer, target_type: order, forward_route_name: orders }
  - { link_type: has_item, source_type: order, target_type: order_item, forward_route_name: items }
  - { link_type: has_invoice, source_type: order, target_type: invoice, forward_route_name: invoices }
  - { link_type: has_payment, source_type: invoice, target_type: payment, forward_route_name: payments }
  - { link_type: has_shipment, source_type: order, target_type: shipment, forward_route_name: shipments }
  - { link_type: has_tracking, source_type: shipment, target_type: tracking_event, forward_route_name: tracking }
```

**Navigation**:
```bash
# Customer journey
GET /customers/cust-1/orders
GET /orders/ord-123/items
GET /orders/ord-123/invoices
GET /invoices/inv-456/payments

# Fulfillment tracking  
GET /orders/ord-123/shipments
GET /shipments/ship-789/tracking
```

---

## 📚 Related Documentation

- [Enriched Links](ENRICHED_LINKS.md)
- [Getting Started](GETTING_STARTED.md)
- [Architecture](../architecture/ARCHITECTURE.md)

---

**Multi-level navigation makes complex data relationships simple and intuitive!** 🚀🌳✨
